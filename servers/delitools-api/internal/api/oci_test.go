//go:build oci

// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestOCIStartupAndFixForward(t *testing.T) {
	root, e := filepath.Abs("../../../..")
	if e != nil {
		t.Fatal(e)
	}
	run := func(args ...string) string {
		t.Helper()
		b, e := exec.Command("docker", args...).CombinedOutput()
		if e != nil {
			t.Fatalf("docker %s: %v\n%s", args[0], e, b)
		}
		return strings.TrimSpace(string(b))
	}
	image := "delitools-api:validation"
	if run("image", "inspect", "--format", "{{.Config.User}}", image) != "65532:65532" {
		t.Fatal("image must be nonroot")
	}
	name := "delitools-oci-" + strings.ReplaceAll(ID(), "-", "")
	run("run", "-d", "--name", name, "-e", "POSTGRES_PASSWORD=fixture-only", "-e", "POSTGRES_DB=delitools", "postgres:17")
	t.Cleanup(func() { exec.Command("docker", "rm", "-f", name).Run() })
	ready := false
	for i := 0; i < 60; i++ {
		if exec.Command("docker", "exec", name, "pg_isready", "-U", "postgres").Run() == nil {
			ready = true
			break
		}
		time.Sleep(time.Second)
	}
	if !ready {
		t.Fatal("PostgreSQL did not start")
	}
	dir := t.TempDir()
	if e = os.Chmod(dir, 0755); e != nil {
		t.Fatal(e)
	}
	arch := run("image", "inspect", "--format", "{{.Architecture}}", "postgres:17")
	build := exec.Command("go", "build", "-o", filepath.Join(dir, "provider"), "./servers/delcore-api/internal/testprovider")
	build.Dir = root
	build.Env = append(os.Environ(), "GOOS=linux", "GOARCH="+arch, "CGO_ENABLED=0")
	if b, e := build.CombinedOutput(); e != nil {
		t.Fatal(e, string(b))
	}
	provider := run("run", "-d", "--network", "container:"+name, "-v", dir+":/fixture:ro", "--entrypoint", "/fixture/provider", image)
	t.Cleanup(func() { exec.Command("docker", "rm", "-f", provider).Run() })
	data, e := os.ReadFile(filepath.Join(root, "servers/delitools-api/config.example.json"))
	if e != nil {
		t.Fatal(e)
	}
	var c map[string]any
	if e = json.Unmarshal(data, &c); e != nil {
		t.Fatal(e)
	}
	c["listen"] = "0.0.0.0:46322"
	c["issuer"] = "http://127.0.0.1:8099"
	c["token_url"] = "http://127.0.0.1:8099/token"
	c["delcore_url"] = "http://127.0.0.1:8099"
	c["gateway_url"] = "http://127.0.0.1:8099/v1"
	save := func() {
		t.Helper()
		b, _ := json.Marshal(c)
		if e = os.WriteFile(filepath.Join(dir, "config.json"), b, 0644); e != nil {
			t.Fatal(e)
		}
	}
	save()
	base := []string{"--network", "container:" + name, "-v", dir + ":/fixture:ro", "-e", "DELITOOLS_DATABASE_URL=postgres://postgres:fixture-only@127.0.0.1:5432/delitools?sslmode=disable", "-e", "DELITOOLS_CLIENT_SECRET=fixture", "-e", "DELITOOLS_GATEWAY_KEY=fixture"}
	args := func(detach bool, extra ...string) []string {
		out := []string{"run"}
		if detach {
			out = append(out, "-d")
		} else {
			out = append(out, "--rm")
		}
		out = append(out, base...)
		out = append(out, image, "-config", "/fixture/config.json", "-log-json")
		return append(out, extra...)
	}
	if b, e := exec.Command("docker", args(false)...).CombinedOutput(); e == nil {
		t.Fatal("startup without migration succeeded", string(b))
	}
	run(args(false, "-migrate")...)
	app := run(args(true)...)
	t.Cleanup(func() { exec.Command("docker", "rm", "-f", app).Run() })
	time.Sleep(2 * time.Second)
	if run("inspect", "--format", "{{.State.Running}}", app) != "true" {
		t.Fatal(run("logs", app))
	}
	// All HTTP traffic remains inside the fixture database container's namespace.
	run("run", "--rm", "--network", "container:"+name, "curlimages/curl:8.18.0", "-fsS", "http://127.0.0.1:46322/readyz")
	metrics := run("run", "--rm", "--network", "container:"+name, "curlimages/curl:8.18.0", "-fsS", "http://127.0.0.1:46322/metrics")
	if !strings.Contains(metrics, "delitools_pending_usage") {
		t.Fatal("metrics missing")
	}
	if _, e := exec.Command("docker", args(false)...).CombinedOutput(); e == nil {
		t.Fatal("second instance accepted")
	}
	run("stop", "--time", "10", app)
	run(args(false, "-migrate")...)
	c["settings"].(map[string]any)["maxInputBytes"] = 0
	save()
	if _, e := exec.Command("docker", args(false)...).CombinedOutput(); e == nil {
		t.Fatal("invalid settings accepted")
	}
	c["settings"].(map[string]any)["maxInputBytes"] = 65536
	save()
	run("exec", name, "psql", "-U", "postgres", "-d", "delitools", "-c", "UPDATE delitools_schema SET version=version+99")
	if _, e := exec.Command("docker", args(false)...).CombinedOutput(); e == nil {
		t.Fatal("incompatible schema accepted")
	}
}
