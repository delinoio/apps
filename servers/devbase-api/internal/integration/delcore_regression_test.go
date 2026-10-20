// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/rsa"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"math/big"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"connectrpc.com/connect"
	dc "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/delinoio/apps/servers/devbase-api/internal/delcore"
	"github.com/golang-jwt/jwt/v4"
	"github.com/jackc/pgx/v5"
)

// realDelcore runs the actual Delcore server binary against its own disposable
// database. A local OpenID provider fixture signs the tokens Delcore verifies.
type realDelcore struct {
	t        *testing.T
	url      string
	audience string
	key      *rsa.PrivateKey
	issuer   string
	db       *pgx.Conn
	process  *exec.Cmd
	client   delcorev1connect.DelcoreServiceClient
}

const operatorRole = "delcore-operator"

func (d *realDelcore) token(subject string, claims jwt.MapClaims) string {
	d.t.Helper()
	all := jwt.MapClaims{"iss": d.issuer, "sub": subject, "aud": d.audience, "exp": time.Now().Add(time.Hour).Unix(), "iat": time.Now().Unix()}
	for k, v := range claims {
		all[k] = v
	}
	token := jwt.NewWithClaims(jwt.SigningMethodRS256, all)
	token.Header["kid"] = "fixture"
	signed, e := token.SignedString(d.key)
	if e != nil {
		d.t.Fatal(e)
	}
	return signed
}

func (d *realDelcore) user(name string) string {
	return d.token("sub-"+name, jwt.MapClaims{"email": name + "@example.test", "email_verified": true})
}

func (d *realDelcore) operator() string {
	return d.token("sub-operator", jwt.MapClaims{"email": "operator@example.test", "email_verified": true, "roles": []string{operatorRole}})
}

func (d *realDelcore) service() string {
	return d.token("devbase-service", jwt.MapClaims{"client_id": "devbase-service", "scope": "authorize"})
}

func call[Q any](token string, message *Q) *connect.Request[Q] {
	r := connect.NewRequest(message)
	r.Header().Set("Authorization", "Bearer "+token)
	return r
}

func startDelcore(t *testing.T) *realDelcore {
	t.Helper()
	root := repositoryRoot(t)
	dsn := os.Getenv("TEST_DATABASE_URL")
	if dsn == "" {
		t.Skip("TEST_DATABASE_URL is required")
	}
	if _, e := exec.LookPath("go"); e != nil {
		t.Skip("go is not installed")
	}
	d := &realDelcore{t: t, audience: "https://delcore.fixture.test"}
	var e error
	if d.key, e = rsa.GenerateKey(rand.Reader, 2048); e != nil {
		t.Fatal(e)
	}
	// Local OpenID provider fixture.
	issuerMux := http.NewServeMux()
	issuerServer := httptest.NewServer(issuerMux)
	t.Cleanup(issuerServer.Close)
	d.issuer = issuerServer.URL
	issuerMux.HandleFunc("/.well-known/openid-configuration", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]any{"issuer": d.issuer, "jwks_uri": d.issuer + "/jwks", "authorization_endpoint": d.issuer + "/auth", "token_endpoint": d.issuer + "/token", "id_token_signing_alg_values_supported": []string{"RS256"}})
	})
	issuerMux.HandleFunc("/jwks", func(w http.ResponseWriter, r *http.Request) {
		public := d.key.PublicKey
		json.NewEncoder(w).Encode(map[string]any{"keys": []map[string]string{{
			"kty": "RSA", "kid": "fixture", "alg": "RS256", "use": "sig",
			"n": base64.RawURLEncoding.EncodeToString(public.N.Bytes()),
			"e": base64.RawURLEncoding.EncodeToString(big.NewInt(int64(public.E)).Bytes()),
		}}})
	})

	// Dedicated database so Delcore's schema never mixes with DevBase's.
	admin, e := pgx.Connect(context.Background(), dsn)
	if e != nil {
		t.Fatal(e)
	}
	name := "delcore_regression_" + strings.ReplaceAll(newID(), "-", "")[:16]
	if _, e = admin.Exec(context.Background(), "CREATE DATABASE "+name); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() {
		admin.Exec(context.Background(), "DROP DATABASE IF EXISTS "+name+" WITH (FORCE)")
		admin.Close(context.Background())
	})
	parsed, e := url.Parse(dsn)
	if e != nil {
		t.Fatal(e)
	}
	parsed.Path = "/" + name
	if d.db, e = pgx.Connect(context.Background(), parsed.String()); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { d.db.Close(context.Background()) })

	listener, e := net.Listen("tcp", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	address := listener.Addr().String()
	listener.Close()
	d.url = "http://" + address
	dir := t.TempDir()
	// Delcore reads its settings from the process environment. The working directory has no
	// .env file, so only these values apply.
	settings := map[string]string{
		"LISTEN": address, "DATABASE_URL": parsed.String(), "ISSUER": d.issuer, "AUDIENCE": d.audience, "OPERATOR_ROLE": operatorRole,
		"IDENTITY_KEY": strings.Repeat("k", 40), "PADDLE_URL": d.issuer, "PADDLE_CREDIT_PRODUCT_ID": "pro_fixture", "PADDLE_TOKEN": "fixture",
		"PADDLE_WEBHOOK_SECRET": strings.Repeat("w", 20), "EMAIL_URL": d.issuer, "EMAIL_TOKEN": "fixture", "EMAIL_ACCOUNT": "fixture", "EMAIL_SENDER": "fixture@example.test",
		"RETURN_URLS": `["https://consumer.example.test/return"]`, "TIMEOUT_SECONDS": "10", "RETRY_LIMIT": "3", "RETRY_SECONDS": "1", "MAX_PAYLOAD_BYTES": "1048576",
		"REQUESTS_PER_MINUTE": "1000000", "INVITATION_SECONDS": "3600", "USAGE_LATENESS_SECONDS": "86400", "REPORTING_GRACE_SECONDS": "86400", "DELETION_GRACE_SECONDS": "86400",
		"RAW_USAGE_RETENTION_SECONDS": "864000", "ACCOUNTING_RETENTION_SECONDS": "8640000", "AUDIT_RETENTION_SECONDS": "8640000", "WEBHOOK_RETENTION_SECONDS": "864000",
		"BACKLOG_ALERT": "1000", "SETTLEMENT_ALERT_SECONDS": "86400", "DEVELOPMENT": "true",
	}
	environment := os.Environ()
	for key, value := range settings {
		environment = append(environment, key+"="+value)
	}
	binary := filepath.Join(dir, "delcore-api")
	build := exec.Command("go", "build", "-o", binary, "./servers/delcore-api/cmd/delcore-api")
	build.Dir = root
	build.Env = append(os.Environ(), "GOWORK=off")
	if out, e := build.CombinedOutput(); e != nil {
		t.Fatalf("building Delcore: %v\n%s", e, out)
	}
	migrate := exec.Command(binary, "-migrate")
	migrate.Dir, migrate.Env = dir, environment
	if out, e := migrate.CombinedOutput(); e != nil {
		t.Fatalf("migrating Delcore: %v\n%s", e, out)
	}
	var logs bytes.Buffer
	d.process = exec.Command(binary)
	d.process.Dir, d.process.Env = dir, environment
	d.process.Stdout, d.process.Stderr = &logs, &logs
	if e = d.process.Start(); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { d.process.Process.Kill(); d.process.Wait() })
	ready := false
	for i := 0; i < 100 && !ready; i++ {
		if resp, e := http.Get(d.url + "/readyz"); e == nil {
			ready = resp.StatusCode == http.StatusOK
			resp.Body.Close()
		}
		time.Sleep(100 * time.Millisecond)
	}
	if !ready {
		t.Fatalf("Delcore did not become ready:\n%s", logs.String())
	}
	d.client = delcorev1connect.NewDelcoreServiceClient(http.DefaultClient, d.url)
	return d
}

// authority returns DevBase's production Delcore client wired to the real server.
func (d *realDelcore) authority() delcore.Authority {
	return &delcore.Client{
		RPC:     d.client,
		Token:   func(context.Context) (string, error) { return d.service(), nil },
		Timeout: 5 * time.Second,
	}
}

func (d *realDelcore) must(e error) {
	d.t.Helper()
	if e != nil {
		d.t.Fatal(e)
	}
}

// TestRealDelcoreRegression exercises DevBase against the actual Delcore server:
// roles, invitations, operator blocking, deletion and billing independence.
func TestRealDelcoreRegression(t *testing.T) {
	d := startDelcore(t)
	ctx := context.Background()
	operator := d.operator()
	reason := "register DevBase regression fixture"
	_, e := d.client.PutPlan(ctx, call(operator, &dc.PutPlanRequest{RequestKey: newID(), Reason: reason, Plan: &dc.Plan{Name: "free", Free: true, DefaultFree: true}}))
	d.must(e)
	_, e = d.client.PutService(ctx, call(operator, &dc.PutServiceRequest{RequestKey: newID(), Reason: reason, Service: &dc.Service{Name: "DevBase", ClientId: "devbase-service", Capabilities: []string{"use"}, Scopes: []string{"authorize"}}}))
	d.must(e)

	// An organization with an Owner, an Admin, a Member and a Billing user.
	owner := d.user("owner")
	created, e := d.client.CreateTenant(ctx, call(owner, &dc.CreateTenantRequest{RequestKey: newID(), Name: "Regression Org"}))
	d.must(e)
	tenant := created.Msg.Tenant.Id
	for name, role := range map[string]dc.Role{"admin": dc.Role_ROLE_ADMIN, "billing": dc.Role_ROLE_BILLING, "member": dc.Role_ROLE_MEMBER} {
		invited, e := d.client.CreateInvitation(ctx, call(owner, &dc.CreateInvitationRequest{RequestKey: newID(), TenantId: tenant, Email: name + "@example.test", Roles: []dc.Role{role}, ReturnUrl: "https://consumer.example.test/return"}))
		d.must(e)
		var text string
		d.must(d.db.QueryRow(ctx, "SELECT payload->>'text' FROM jobs WHERE dedup_key=$1", "invitation:"+invited.Msg.InvitationId).Scan(&text))
		link, e := url.Parse(text[strings.Index(text, "https://"):])
		d.must(e)
		_, e = d.client.AcceptInvitation(ctx, call(d.user(name), &dc.AcceptInvitationRequest{RequestKey: newID(), Token: link.Query().Get("invitation_token")}))
		d.must(e)
	}

	f := setupWith(t, d.authority())
	api := func(user string) string { return d.user(user) }
	project, e := f.client.CreateProject(ctx, as(api("owner"), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "Real project"}))
	d.must(e)
	p := project.Msg.Project
	// Owner and Admin manage; Billing, Member, a stranger and invalid tokens do not.
	if _, e = f.client.GetProject(ctx, as(api("admin"), &pb.GetProjectRequest{ProjectId: p.Id})); e != nil {
		t.Fatal("admin denied", e)
	}
	for _, user := range []string{"billing", "member", "stranger"} {
		if _, e = f.client.GetProject(ctx, as(api(user), &pb.GetProjectRequest{ProjectId: p.Id})); code(e) != connect.CodePermissionDenied {
			t.Fatalf("%s was not denied: %v", user, e)
		}
	}
	for name, token := range map[string]string{"garbage": "garbage", "expired": d.token("sub-owner", jwt.MapClaims{"exp": time.Now().Add(-time.Hour).Unix(), "email": "owner@example.test", "email_verified": true}), "service": d.service()} {
		if _, e = f.client.GetProject(ctx, as(token, &pb.GetProjectRequest{ProjectId: p.Id})); code(e) != connect.CodeUnauthenticated && code(e) != connect.CodePermissionDenied {
			t.Fatalf("%s token accepted: %v", name, e)
		}
	}
	// Unknown or other tenants are denied.
	personal, e := d.client.GetCurrentUser(ctx, call(api("stranger"), &dc.GetCurrentUserRequest{}))
	d.must(e)
	if _, e = f.client.ListProjects(ctx, as(api("owner"), &pb.ListProjectsRequest{TenantId: personal.Msg.Tenants[0].Id})); code(e) != connect.CodePermissionDenied {
		t.Fatal("another user's personal tenant was readable", e)
	}
	if _, e = f.client.ListProjects(ctx, as(api("owner"), &pb.ListProjectsRequest{TenantId: newID()})); code(e) != connect.CodePermissionDenied {
		t.Fatal("unknown tenant was readable", e)
	}

	secret, apiKey := f.key(api("admin"), p.Id, crud)
	user, e := f.client.CreateEndUser(ctx, as(secret, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "real-1"}))
	d.must(e)

	// Billing conditions never restrict this free feature.
	_, e = d.db.Exec(ctx, "UPDATE tenants SET renewal_failed=true WHERE id=$1", tenant)
	d.must(e)
	if _, e = f.client.GetEndUser(ctx, as(secret, &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: user.Msg.EndUser.Id})); e != nil {
		t.Fatal("a billing condition restricted the free feature", e)
	}

	// Operator blocking denies keys but keeps the management exceptions.
	_, e = d.client.SetTenantBlocked(ctx, call(operator, &dc.SetTenantBlockedRequest{RequestKey: newID(), TenantId: tenant, Blocked: true, Reason: reason}))
	d.must(e)
	if _, e = f.client.GetEndUser(ctx, as(secret, &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: user.Msg.EndUser.Id})); code(e) != connect.CodePermissionDenied {
		t.Fatal("blocked tenant's key worked", e)
	}
	if _, e = f.client.GetProject(ctx, as(api("owner"), &pb.GetProjectRequest{ProjectId: p.Id})); e != nil {
		t.Fatal("management read denied while blocked", e)
	}
	if _, e = f.client.CreateProject(ctx, as(api("owner"), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "Blocked create"})); code(e) != connect.CodePermissionDenied {
		t.Fatal("creation allowed while blocked", e)
	}
	if _, e = f.client.DeleteEndUser(ctx, as(api("admin"), &pb.DeleteEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: user.Msg.EndUser.Id, Version: 1})); e != nil {
		t.Fatal("user deletion denied while blocked", e)
	}
	_, e = d.client.SetTenantBlocked(ctx, call(operator, &dc.SetTenantBlockedRequest{RequestKey: newID(), TenantId: tenant, Blocked: false, Reason: reason}))
	d.must(e)
	if _, e = f.client.ListEndUsers(ctx, as(secret, &pb.ListEndUsersRequest{ProjectId: p.Id})); e != nil {
		t.Fatal("unblocked tenant's key denied", e)
	}

	// Deletion: pending deletion denies keys and permits only the exceptions.
	_, e = d.db.Exec(ctx, "UPDATE tenants SET deleting_at=now() WHERE id=$1", tenant)
	d.must(e)
	if _, e = f.client.ListEndUsers(ctx, as(secret, &pb.ListEndUsersRequest{ProjectId: p.Id})); code(e) != connect.CodePermissionDenied {
		t.Fatal("deleting tenant's key worked", e)
	}
	if _, e = f.client.RevokeApiKey(ctx, as(api("owner"), &pb.RevokeApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, ApiKeyId: apiKey.Id})); e != nil {
		t.Fatal("revocation denied while deleting", e)
	}
	// Pending deletion never authorizes cleanup.
	f.worker().Cycle(ctx)
	if f.count("SELECT count(*) FROM projects") != 1 {
		t.Fatal("cleanup ran before final deletion")
	}
	// Final deletion removes the data and nothing else.
	other, e := f.client.CreateProject(ctx, as(api("stranger"), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: personal.Msg.Tenants[0].Id, Name: "Survivor"}))
	d.must(e)
	_, e = d.db.Exec(ctx, "UPDATE tenants SET deleted_at=now() WHERE id=$1", tenant)
	d.must(e)
	f.worker().Cycle(ctx)
	if f.count("SELECT count(*) FROM projects WHERE tenant_id=$1", tenant) != 0 {
		t.Fatal("finalized tenant's projects remain")
	}
	if f.count("SELECT count(*) FROM projects WHERE id=$1", other.Msg.Project.Id) != 1 {
		t.Fatal("cleanup removed an unrelated tenant's project")
	}

	// A Delcore outage denies access and commits nothing.
	d.process.Process.Kill()
	d.process.Wait()
	before := f.count("SELECT count(*) FROM projects")
	_, e = f.client.CreateProject(ctx, as(api("stranger"), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: personal.Msg.Tenants[0].Id, Name: fmt.Sprint("During outage")}))
	if code(e) != connect.CodeUnavailable || f.count("SELECT count(*) FROM projects") != before {
		t.Fatal("outage did not fail closed", e)
	}
}
