// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build oci

package integration

import (
	"os/exec"
	"strings"
	"testing"
	"time"
)

// TestOCI exercises the built image against disposable PostgreSQL: explicit
// migration, schema compatibility, readiness, the single-instance lock and
// graceful shutdown. It needs a prior `docker build -t devbase-api:validation`.
func TestOCI(t *testing.T) {
	run := func(args ...string) string {
		t.Helper()
		b, e := exec.Command("docker", args...).CombinedOutput()
		if e != nil {
			t.Fatalf("docker %s: %v\n%s", args[0], e, b)
		}
		return strings.TrimSpace(string(b))
	}
	fails := func(args ...string) string {
		t.Helper()
		b, e := exec.Command("docker", args...).CombinedOutput()
		if e == nil {
			t.Fatalf("expected failure: %s", b)
		}
		return string(b)
	}
	image := "devbase-api:validation"
	if user := run("image", "inspect", "--format", "{{.Config.User}}", image); user != "65532:65532" {
		t.Fatal("image must run as nonroot", user)
	}
	name := "devbase-oci-" + strings.ReplaceAll(time.Now().UTC().Format("150405.000000000"), ".", "")
	run("run", "-d", "--name", name, "-e", "POSTGRES_PASSWORD=fixture-only", "-e", "POSTGRES_DB=devbase", "postgres:17")
	t.Cleanup(func() { exec.Command("docker", "rm", "-f", name).Run() })
	ready := false
	for i := 0; i < 60; i++ {
		if exec.Command("docker", "exec", name, "pg_isready", "-U", "postgres", "-d", "devbase").Run() == nil {
			ready = true
			break
		}
		time.Sleep(time.Second)
	}
	if !ready {
		t.Fatal("PostgreSQL did not start")
	}
	// The server container shares the database container's network namespace.
	env := []string{
		"-e", "DATABASE_URL=postgres://postgres:fixture-only@127.0.0.1:5432/devbase?sslmode=disable",
		"-e", "DELCORE_URL=http://127.0.0.1:9", "-e", "DELCORE_RESOURCE=https://delcore.example.invalid",
		"-e", "LOGTO_TOKEN_URL=http://127.0.0.1:9/token", "-e", "CLIENT_ID=fixture", "-e", "CLIENT_SECRET=fixture",
		"-e", "NO_COLOR=1",
	}
	args := func(command ...string) []string {
		a := append([]string{"run", "--rm", "--network", "container:" + name}, env...)
		return append(append(a, image), command...)
	}
	fails("run", "--rm", image)    // Missing configuration.
	fails(args("check-schema")...) // No schema yet.
	fails(args("serve")...)        // Startup never migrates implicitly.
	run(args("migrate")...)
	run(args("migrate")...) // Repeating is safe.
	run(args("check-schema")...)
	// A modified applied migration fails both the migration command and the startup check.
	run("exec", name, "psql", "-U", "postgres", "-d", "devbase", "-c", "UPDATE devbase_schema SET digest='tampered'")
	fails(args("check-schema")...)
	fails(args("migrate")...)
	run("exec", name, "psql", "-U", "postgres", "-d", "devbase", "-c", "DELETE FROM devbase_schema")
	fails(args("check-schema")...)
	run("exec", name, "psql", "-U", "postgres", "-d", "devbase", "-c", "DROP SCHEMA public CASCADE; CREATE SCHEMA public")
	run(args("migrate")...)

	app := name + "-app"
	up := append([]string{"run", "-d", "--name", app, "--network", "container:" + name}, env...)
	run(append(up, image)...)
	t.Cleanup(func() { exec.Command("docker", "rm", "-f", app).Run() })
	started := false
	for i := 0; i < 30; i++ {
		if strings.Contains(run("logs", app), "devbase-api listening") {
			started = true
			break
		}
		time.Sleep(time.Second)
	}
	if !started {
		t.Fatalf("application did not start: %s", run("logs", app))
	}
	status := func(path string) string {
		return run("exec", name, "bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/46317; printf 'GET "+path+" HTTP/1.0\\r\\n\\r\\n' >&3; head -n1 <&3")
	}
	if got := status("/healthz"); !strings.Contains(got, "200") {
		t.Fatal("health", got)
	}
	if got := status("/readyz"); !strings.Contains(got, "200") {
		t.Fatal("readiness", got)
	}
	fails(args("serve")...) // The database lock rejects another instance.
	run("exec", name, "psql", "-U", "postgres", "-d", "devbase", "-c", "UPDATE devbase_schema SET digest='tampered'")
	if got := status("/readyz"); !strings.Contains(got, "503") {
		t.Fatal("readiness must fail on an incompatible schema", got)
	}
	run("stop", "--time", "15", app)
	if code := run("inspect", "--format", "{{.State.ExitCode}}", app); code != "0" {
		t.Fatal("unclean shutdown", code)
	}
}
