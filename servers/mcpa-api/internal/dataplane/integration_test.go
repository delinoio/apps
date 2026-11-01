// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package dataplane

import (
	"bufio"
	"bytes"
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"connectrpc.com/connect"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/prometheus/client_golang/prometheus/testutil"
	"github.com/tidwall/gjson"
	"google.golang.org/protobuf/types/known/timestamppb"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/pricing"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

type formatCase struct {
	name, path, body string
	header           map[string]string
}

func formatCases() []formatCase {
	claude := map[string]string{"Anthropic-Version": "2023-06-01"}
	gemini := `{"contents":[{"role":"user","parts":[{"text":"` + promptCanary + `"}]}]}`
	return []formatCase{
		{"openai-chat", "/v1/chat/completions", chatBody(false), nil},
		{"openai-chat-stream", "/v1/chat/completions", chatBody(true), nil},
		{"openai-responses", "/v1/responses", `{"model":"compat-model","input":"` + promptCanary + `"}`, nil},
		{"openai-responses-stream", "/v1/responses", `{"model":"compat-model","stream":true,"input":"` + promptCanary + `"}`, nil},
		{"claude", "/v1/messages", `{"model":"compat-model","max_tokens":16,"messages":[{"role":"user","content":"` + promptCanary + `"}]}`, claude},
		{"claude-stream", "/v1/messages", `{"model":"compat-model","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"` + promptCanary + `"}]}`, claude},
		{"gemini", "/v1beta/models/compat-model:generateContent", gemini, nil},
		{"gemini-stream", "/v1beta/models/compat-model:streamGenerateContent?alt=sse", gemini, nil},
	}
}

func TestIntegrationHappyPath(t *testing.T) {
	e := newEnv(t)
	id := e.resource("happy")
	key, keyID := e.callKey("owner", id)

	var calls []string
	for _, tc := range formatCases() {
		res := e.call(e.srv.URL, id, key, tc.path, tc.body, tc.header)
		if res.status != http.StatusOK || !strings.Contains(string(res.body), outputCanary) {
			t.Fatalf("%s: status %d body %s", tc.name, res.status, res.body)
		}
		rec := e.waitFinal(res.callID)
		if rec.Status != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED || rec.Provider != mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE ||
			rec.Model != "compat-model" || rec.RequestBytes <= 0 || rec.ResponseBytes <= 0 || rec.CallKeyID.String() != keyID {
			t.Fatalf("%s: record %+v", tc.name, rec)
		}
		calls = append(calls, res.callID)
	}
	if got := e.up.hits.Load(); got != int64(len(calls)) {
		t.Fatalf("upstream hits %d, want %d", got, len(calls))
	}
	e.up.mu.Lock()
	auths := append([]string(nil), e.up.auths...)
	e.up.mu.Unlock()
	for _, auth := range auths {
		if auth != "Bearer "+upstreamAPIKey {
			t.Fatalf("upstream saw %q", auth)
		}
	}
	if got := testutil.ToFloat64(e.app.Metrics.HTTPRequests.WithLabelValues("proxy", "2xx")); got != float64(len(calls)) {
		t.Fatalf("proxy requests counted %v times for %d calls", got, len(calls))
	}

	total := e.scalar(`SELECT COALESCE(sum(request_bytes + response_bytes), 0)::bigint FROM call_records WHERE resource_id = $1`, id)
	reqBytes := e.scalar(`SELECT COALESCE(sum(request_bytes), 0)::bigint FROM call_records WHERE resource_id = $1`, id)

	// An invalid key is neither recorded nor billed.
	before := e.scalar(`SELECT count(*) FROM call_records`)
	if res := e.call(e.srv.URL, id, "mcpa_invalid", "/v1/chat/completions", chatBody(false), nil); res.status != http.StatusUnauthorized ||
		gjson.GetBytes(res.body, "error.type").String() != "invalid_call_key" {
		t.Fatalf("invalid key: %d %s", res.status, res.body)
	}
	if e.scalar(`SELECT count(*) FROM call_records`) != before || e.scalar(`SELECT count(*) FROM usage_outbox WHERE resource_id <> $1`, id) != 0 {
		t.Fatal("an invalid-key call was recorded")
	}

	// Usage view: the summary and the rows match the call records.
	usage := mustDo(e, "owner", e.rpc.GetUsage, &mcpav1.GetUsageRequest{TenantId: e.tenant, Filter: &mcpav1.CallFilter{ResourceIds: []string{id}}})
	s := usage.GetSummary()
	if s.GetCallCount() != int64(len(calls)) || s.GetTotalBytes() != total || s.GetRequestBytes() != reqBytes ||
		s.GetTotalGib() != pricing.GiB(total) || s.GetEstimatedUsd() != pricing.EstimateUSD(total) {
		t.Fatalf("summary %+v, want %d bytes", s, total)
	}
	var rowBytes, rowCalls int64
	for _, r := range usage.GetRows() {
		rowBytes += r.GetRequestBytes() + r.GetResponseBytes()
		rowCalls += r.GetCallCount()
		if r.GetStatus() != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED || r.GetModel() != "compat-model" || r.GetCallKeyId() != keyID {
			t.Fatalf("row %+v", r)
		}
	}
	if rowBytes != total || rowCalls != int64(len(calls)) {
		t.Fatalf("rows sum %d bytes / %d calls, want %d / %d", rowBytes, rowCalls, total, len(calls))
	}
	// Filters.
	for _, f := range []*mcpav1.CallFilter{
		{ResourceIds: []string{id}, Models: []string{"other"}},
		{ResourceIds: []string{id}, Providers: []mcpav1.Provider{mcpav1.Provider_PROVIDER_CLAUDE}},
		{ResourceIds: []string{id}, Statuses: []mcpav1.CallStatus{mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR}},
		{ResourceIds: []string{id}, CallKeyIds: []string{newKey()}},
		{ResourceIds: []string{id}, StartTime: timestamppb.New(time.Now().Add(48 * time.Hour)), EndTime: timestamppb.New(time.Now().Add(72 * time.Hour))},
	} {
		got := mustDo(e, "owner", e.rpc.GetUsage, &mcpav1.GetUsageRequest{TenantId: e.tenant, Filter: f})
		if got.GetSummary().GetTotalBytes() != 0 || len(got.GetRows()) != 0 {
			t.Fatalf("filter %v matched %+v", f, got.GetSummary())
		}
	}
	matched := mustDo(e, "owner", e.rpc.GetUsage, &mcpav1.GetUsageRequest{TenantId: e.tenant, Filter: &mcpav1.CallFilter{
		Models: []string{"compat-model"}, Providers: []mcpav1.Provider{mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE},
		Statuses: []mcpav1.CallStatus{mcpav1.CallStatus_CALL_STATUS_SUCCEEDED}, CallKeyIds: []string{keyID},
		StartTime: timestamppb.New(time.Now().Add(-time.Hour)), EndTime: timestamppb.New(time.Now().Add(time.Hour)),
	}})
	if matched.GetSummary().GetTotalBytes() != total {
		t.Fatalf("tenant-wide filtered usage %d, want %d", matched.GetSummary().GetTotalBytes(), total)
	}

	// Call records page through every call, newest first.
	seen := map[string]bool{}
	after := ""
	for {
		page := mustDo(e, "owner", e.rpc.ListCallRecords, &mcpav1.ListCallRecordsRequest{
			TenantId: e.tenant, Filter: &mcpav1.CallFilter{ResourceIds: []string{id}}, Page: &mcpav1.Page{Size: 3, AfterId: after},
		})
		for _, r := range page.GetRecords() {
			if seen[r.GetId()] || r.GetRequestId() != r.GetId() || r.GetLatencyMs() < 0 {
				t.Fatalf("record %+v", r)
			}
			seen[r.GetId()] = true
		}
		if after = page.GetNextAfterId(); after == "" {
			break
		}
	}
	if len(seen) != len(calls) {
		t.Fatalf("listed %d records, want %d", len(seen), len(calls))
	}
	// Invokers cannot read call records; they can read usage.
	mustDo(e, "owner", e.rpc.PutAclEntry, &mcpav1.PutAclEntryRequest{ResourceId: id, PrincipalKind: mcpav1.PrincipalKind_PRINCIPAL_KIND_USER,
		PrincipalId: e.users["invoker"], Role: mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE})
	if got := mustDo(e, "invoker", e.rpc.ListCallRecords, &mcpav1.ListCallRecordsRequest{TenantId: e.tenant}); len(got.GetRecords()) != 0 {
		t.Fatal("an invoker read call records")
	}
	if got := mustDo(e, "invoker", e.rpc.GetUsage, &mcpav1.GetUsageRequest{TenantId: e.tenant}); got.GetSummary().GetTotalBytes() != total {
		t.Fatal("an invoker cannot read usage")
	}

	// The outbox holds exactly the billed bytes; delivery sends matching
	// IngestEvent calls and empties it.
	if got := e.scalar(`SELECT COALESCE(sum(bytes), 0)::bigint FROM usage_outbox`); got != total {
		t.Fatalf("outbox holds %d bytes, want %d", got, total)
	}
	res, err := e.dp.Outbox.Cycle(e.ctx)
	if err != nil || res.Retried != 0 || res.Poisoned != 0 {
		t.Fatalf("cycle %+v %v", res, err)
	}
	var delivered int64
	for _, ev := range e.dc.ingested() {
		bytes, err := strconv.ParseInt(ev.GetAttributes()["bytes"].GetNumber(), 10, 64)
		if err != nil || bytes <= 0 || ev.GetEventType() != "mcpa.traffic" || ev.GetTenantId() != e.tenant ||
			ev.GetAttributes()["resource_id"].GetText() != id || ev.GetServiceId() != e.vars["MCPA_SERVICE_ID"] {
			t.Fatalf("event %v", ev)
		}
		parts := strings.Split(ev.GetEventKey(), ":")
		if len(parts) != 3 || parts[0] != "mcpa" {
			t.Fatalf("event key %q", ev.GetEventKey())
		}
		rec := e.record(parts[1])
		if !ev.GetOccurredAt().AsTime().Equal(rec.StartTime.Truncate(time.Microsecond)) {
			t.Fatalf("occurred_at %v, call start %v", ev.GetOccurredAt().AsTime(), rec.StartTime)
		}
		delivered += bytes
	}
	if delivered != total || e.scalar(`SELECT count(*) FROM usage_outbox`) != 0 {
		t.Fatalf("delivered %d bytes, want %d", delivered, total)
	}
	for _, rk := range e.dc.requests {
		if u, err := uuid.Parse(rk); err != nil || u.Version() != 7 {
			t.Fatalf("request key %q", rk)
		}
	}
	if got := testutil.ToFloat64(e.dp.metrics.outboxPending); got != 0 {
		t.Fatalf("pending gauge %v", got)
	}

	// A rejection after a valid key is recorded and bills its error response.
	mustDo(e, "owner", e.rpc.UpdateRouting, &mcpav1.UpdateRoutingRequest{ResourceId: id, Routing: &mcpav1.RoutingSettings{ModelAllowlist: []string{"other-model"}}})
	rej := e.call(e.srv.URL, id, key, "/v1/chat/completions", chatBody(false), nil)
	if rej.status != http.StatusForbidden || gjson.GetBytes(rej.body, "error.type").String() != "model_not_allowed" {
		t.Fatalf("allowlist: %d %s", rej.status, rej.body)
	}
	rec := e.waitFinal(rej.callID)
	if rec.Status != mcpav1.CallStatus_CALL_STATUS_REJECTED_MODEL_NOT_ALLOWED || rec.RequestBytes != 0 || rec.ResponseBytes <= int64(len(rej.body)) {
		t.Fatalf("rejected record %+v", rec)
	}
	if got := e.scalar(`SELECT COALESCE(sum(bytes), 0)::bigint FROM usage_outbox`); got != rec.ResponseBytes {
		t.Fatalf("rejection billed %d bytes, want %d", got, rec.ResponseBytes)
	}

	assertNoSecretsOrBodies(t, e, key)
}

// assertNoSecretsOrBodies scans every table for prompts, outputs, provider
// keys and call key secrets.
func assertNoSecretsOrBodies(t *testing.T, e *itEnv, secrets ...string) {
	t.Helper()
	rows, err := e.db.Pool.Query(e.ctx, `SELECT tablename FROM pg_tables WHERE schemaname = current_schema()`)
	if err != nil {
		t.Fatal(err)
	}
	tables, err := pgx.CollectRows(rows, pgx.RowTo[string])
	if err != nil {
		t.Fatal(err)
	}
	needles := append([]string{promptCanary, outputCanary, upstreamAPIKey}, secrets...)
	for _, table := range tables {
		rows, err := e.db.Pool.Query(e.ctx, `SELECT row_to_json(t)::text FROM `+pgx.Identifier{table}.Sanitize()+` t`)
		if err != nil {
			t.Fatal(err)
		}
		dump, err := pgx.CollectRows(rows, pgx.RowTo[string])
		if err != nil {
			t.Fatal(err)
		}
		text := strings.Join(dump, "\n")
		for _, n := range needles {
			if strings.Contains(text, n) || strings.Contains(text, hex.EncodeToString([]byte(n))) {
				t.Fatalf("table %s contains %q", table, n)
			}
		}
	}
}

// child is a re-executed mcpa-api process.
type child struct {
	cmd  *exec.Cmd
	base string
	out  *bytes.Buffer
}

func startChild(t *testing.T, e *itEnv, extra map[string]string) *child {
	t.Helper()
	addr := freePort(t)
	cmd := exec.Command(os.Args[0], "-test.run=^$")
	env := append(os.Environ(), childEnv+"=1", "LISTEN="+addr, "MCPA_IT_UPSTREAM="+e.up.srv.URL)
	for k, v := range e.vars {
		if _, override := extra[k]; !override {
			env = append(env, k+"="+v)
		}
	}
	for k, v := range extra {
		env = append(env, k+"="+v)
	}
	cmd.Env = env
	out := &bytes.Buffer{}
	cmd.Stdout, cmd.Stderr = out, out
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	c := &child{cmd: cmd, base: "http://" + addr, out: out}
	t.Cleanup(func() {
		if cmd.ProcessState == nil {
			_ = cmd.Process.Kill()
			_ = cmd.Wait()
		}
		if t.Failed() {
			t.Logf("child output:\n%s", out.String())
		}
	})
	deadline := time.Now().Add(60 * time.Second)
	for time.Now().Before(deadline) {
		resp, err := http.Get(c.base + "/readyz")
		if err == nil {
			resp.Body.Close()
			if resp.StatusCode == http.StatusOK {
				return c
			}
		}
		time.Sleep(100 * time.Millisecond)
	}
	t.Fatalf("child did not become ready:\n%s", out.String())
	return nil
}

// TestIntegrationCrashRecovery kills a serving process mid-stream after a
// checkpoint committed. After a restart, exactly the committed bytes are
// billed, once.
func TestIntegrationCrashRecovery(t *testing.T) {
	e := newEnv(t)
	id := e.resource("crash")
	key, _ := e.callKey("owner", id)
	stall := make(chan struct{})
	defer close(stall)
	e.up.setStall(stall)

	first := startChild(t, e, nil)
	callIDs := make(chan string, 1)
	go func() {
		req, _ := http.NewRequest(http.MethodPost, first.base+"/r/"+id+"/v1/chat/completions", strings.NewReader(chatBody(true)))
		req.Header.Set("Authorization", "Bearer "+key)
		req.Header.Set("Content-Type", "application/json")
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			callIDs <- ""
			return
		}
		callIDs <- resp.Header.Get("X-Mcpa-Request-Id")
		_, _ = io.Copy(io.Discard, bufio.NewReader(resp.Body))
		resp.Body.Close()
	}()
	select {
	case <-e.up.stalls:
	case <-time.After(30 * time.Second):
		t.Fatal("the stream did not reach the upstream")
	}
	callID := <-callIDs
	if callID == "" {
		t.Fatal("no call ID")
	}
	// Wait for a committed checkpoint that covers the first chunk.
	deadline := time.Now().Add(10 * time.Second)
	for {
		var req, resp int64
		err := e.db.Pool.QueryRow(e.ctx, `SELECT request_bytes, response_bytes FROM call_records WHERE id = $1`, callID).Scan(&req, &resp)
		if err == nil && req > 0 && resp > 0 && e.scalar(`SELECT count(*) FROM usage_outbox WHERE event_key LIKE $1`, "mcpa:"+callID+":%") > 0 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("no checkpoint committed")
		}
		time.Sleep(20 * time.Millisecond)
	}
	time.Sleep(300 * time.Millisecond) // later checkpoints have nothing new to commit
	if err := first.cmd.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	_ = first.cmd.Wait()

	rec := e.record(callID)
	committed := rec.RequestBytes + rec.ResponseBytes
	if rec.Final {
		t.Fatal("the killed call has a final record")
	}
	if got := e.scalar(`SELECT COALESCE(sum(bytes), 0)::bigint FROM usage_outbox WHERE event_key LIKE $1`, "mcpa:"+callID+":%"); got != committed {
		t.Fatalf("outbox %d bytes, committed %d", got, committed)
	}
	if n := len(e.dc.ingested()); n != 0 {
		t.Fatalf("%d events delivered before the restart", n)
	}

	// The restarted process delivers the committed checkpoints.
	second := startChild(t, e, map[string]string{"MCPA_OUTBOX_INTERVAL": "200ms"})
	deadline = time.Now().Add(20 * time.Second)
	for e.scalar(`SELECT count(*) FROM usage_outbox`) != 0 {
		if time.Now().After(deadline) {
			t.Fatal("the restarted process did not deliver the outbox")
		}
		time.Sleep(50 * time.Millisecond)
	}
	time.Sleep(600 * time.Millisecond) // more cycles must not resend anything
	_ = second.cmd.Process.Signal(syscall.SIGTERM)
	_ = second.cmd.Wait()

	var delivered int64
	for _, ev := range e.dc.ingested() {
		if !strings.HasPrefix(ev.GetEventKey(), "mcpa:"+callID+":") {
			continue
		}
		n, _ := strconv.ParseInt(ev.GetAttributes()["bytes"].GetNumber(), 10, 64)
		delivered += n
		e.dc.mu.Lock()
		attempts := e.dc.attempts[ev.GetEventKey()]
		e.dc.mu.Unlock()
		if attempts != 1 {
			t.Fatalf("event %s sent %d times", ev.GetEventKey(), attempts)
		}
	}
	if delivered != committed {
		t.Fatalf("billed %d bytes, committed %d", delivered, committed)
	}
	if got := e.scalar(`SELECT COALESCE(sum(request_bytes + response_bytes), 0)::bigint FROM usage_daily WHERE resource_id = $1 AND status = 0`, id); got != committed {
		t.Fatalf("in-progress aggregate %d, committed %d", got, committed)
	}
}

func (e *itEnv) checkpoint(tenant string, req, resp int64) store.Checkpoint {
	e.t.Helper()
	c := store.Checkpoint{
		CallID: store.NewID(), TenantID: tenant, ResourceID: store.NewID(), CallKeyID: store.NewID(),
		Provider: mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE, Model: "m", Format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_OPENAI,
		StartTime: time.Now().Add(-time.Minute), Seq: 1, DeltaRequestBytes: req, DeltaResponseBytes: resp,
		RequestBytes: req, ResponseBytes: resp, OutboxID: store.NewID(), RequestKey: store.NewID(),
	}
	if _, err := e.db.ApplyCheckpoint(e.ctx, c); err != nil {
		e.t.Fatal(err)
	}
	return c
}

type outboxState struct {
	state    int16
	attempts int
	next     time.Time
	last     *time.Time
	category string
}

func (e *itEnv) outboxRow(c store.Checkpoint) (outboxState, bool) {
	var s outboxState
	err := e.db.Pool.QueryRow(e.ctx, `SELECT state, attempts, next_attempt_at, last_attempt_at, last_error FROM usage_outbox WHERE event_key = $1`,
		store.EventKey(c.CallID, c.Seq)).Scan(&s.state, &s.attempts, &s.next, &s.last, &s.category)
	if errors.Is(err, pgx.ErrNoRows) {
		return s, false
	}
	if err != nil {
		e.t.Fatal(err)
	}
	return s, true
}

func TestIntegrationOutboxRetryPoisonAndExpiry(t *testing.T) {
	e := newEnv(t)
	w := e.dp.Outbox
	a, b := e.checkpoint(e.tenant, 100, 50), e.checkpoint(e.tenant, 7, 3)

	// A resent checkpoint is applied once.
	if applied, err := e.db.ApplyCheckpoint(e.ctx, a); err != nil || applied {
		t.Fatalf("resent checkpoint applied=%v err=%v", applied, err)
	}
	if got := e.scalar(`SELECT count(*) FROM usage_outbox`); got != 2 {
		t.Fatalf("%d outbox rows", got)
	}
	// Content is immutable.
	if _, err := e.db.Pool.Exec(e.ctx, `UPDATE usage_outbox SET bytes = 1`); err == nil {
		t.Fatal("outbox content was updated")
	}

	// Delcore is down: rows stay pending with doubling backoff.
	e.dc.set(func(f *fakeDelcore) { f.down = true })
	for attempt, wait := range []time.Duration{0, 10 * time.Second, 20 * time.Second, 40 * time.Second} {
		e.clock.advance(wait)
		res, err := w.Cycle(e.ctx)
		if err != nil || res.Retried != 2 {
			t.Fatalf("attempt %d: %+v %v", attempt+1, res, err)
		}
		s, ok := e.outboxRow(a)
		backoff := Backoff(10*time.Second, attempt+1)
		if !ok || s.state != 1 || s.attempts != attempt+1 || s.category != "unavailable" || s.last == nil || s.next.Sub(*s.last).Round(time.Millisecond) != backoff {
			t.Fatalf("attempt %d: row %+v, want backoff %v", attempt+1, s, backoff)
		}
		// Not due again before the backoff ends.
		if res, _ := w.Cycle(e.ctx); res.Retried != 0 {
			t.Fatalf("attempt %d resent early", attempt+1)
		}
	}
	if got := testutil.ToFloat64(e.dp.metrics.outboxPending); got != 2 {
		t.Fatalf("pending gauge %v", got)
	}
	if got := testutil.ToFloat64(e.dp.metrics.outboxFailures.WithLabelValues("unavailable")); got != 8 {
		t.Fatalf("unavailable failures %v", got)
	}
	if got := testutil.ToFloat64(e.dp.metrics.outboxOldestAge); got <= 0 {
		t.Fatalf("oldest age %v", got)
	}

	// Delcore recovers: each event is delivered once.
	e.dc.set(func(f *fakeDelcore) { f.down = false })
	e.clock.advance(80 * time.Second)
	if res, err := w.Cycle(e.ctx); err != nil || res.Delivered != 2 {
		t.Fatalf("recovery %+v %v", res, err)
	}
	if len(e.dc.ingested()) != 2 || e.scalar(`SELECT count(*) FROM usage_outbox`) != 0 {
		t.Fatal("events were not delivered exactly once")
	}
	got := map[string]string{}
	for _, ev := range e.dc.ingested() {
		got[ev.GetEventKey()] = ev.GetAttributes()["bytes"].GetNumber()
	}
	if got[store.EventKey(a.CallID, 1)] != "150" || got[store.EventKey(b.CallID, 1)] != "10" {
		t.Fatalf("events %v", got)
	}

	// Non-retryable reasons poison the row; it stays until expiry and is
	// never sent again.
	p := e.checkpoint(e.tenant, 1, 1)
	e.dc.set(func(f *fakeDelcore) {
		f.ingestErr = func(*delcorev1.Event) error {
			return reasonError(connect.CodeInvalidArgument, delcorev1.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE)
		}
	})
	if res, _ := w.Cycle(e.ctx); res.Poisoned != 1 {
		t.Fatalf("poison %+v", res)
	}
	if s, ok := e.outboxRow(p); !ok || s.state != 2 || s.category != "invalid_attribute" {
		t.Fatalf("poisoned row %+v", s)
	}
	if got := testutil.ToFloat64(e.dp.metrics.outboxPoisoned); got != 1 {
		t.Fatalf("poisoned gauge %v", got)
	}
	attempts := func(c store.Checkpoint) int {
		e.dc.mu.Lock()
		defer e.dc.mu.Unlock()
		return e.dc.attempts[store.EventKey(c.CallID, c.Seq)]
	}
	e.clock.advance(2 * time.Hour)
	_, _ = w.Cycle(e.ctx)
	if attempts(p) != 1 {
		t.Fatalf("a poisoned row was resent: %d", attempts(p))
	}
	e.dc.set(func(f *fakeDelcore) { f.ingestErr = nil })

	// A pending row older than 30 days is deleted without sending, and so is
	// the poisoned row.
	e.dc.set(func(f *fakeDelcore) { f.down = true })
	late := e.checkpoint(e.tenant, 5, 5)
	e.clock.advance(30*24*time.Hour + time.Minute)
	res, err := w.Cycle(e.ctx)
	if err != nil || res.Expired != 2 {
		t.Fatalf("expiry %+v %v", res, err)
	}
	if attempts(late) != 0 || e.scalar(`SELECT count(*) FROM usage_outbox`) != 0 {
		t.Fatal("an expired row was sent or kept")
	}
	if got := testutil.ToFloat64(e.dp.metrics.outboxFailures.WithLabelValues("expired")); got != 2 {
		t.Fatalf("expired counter %v", got)
	}

	// Rows of tenants Delcore reports DELETED are dropped, not sent.
	e.clock.offset.Store(0)
	e.dc.set(func(f *fakeDelcore) { f.down = false })
	gone := uuid.NewString()
	g := e.checkpoint(gone, 9, 9)
	e.dc.set(func(f *fakeDelcore) { f.states[gone] = delcorev1.TenantState_TENANT_STATE_DELETED })
	if res, _ := w.Cycle(e.ctx); res.TenantDeleted != 1 || attempts(g) != 0 {
		t.Fatalf("deleted tenant %+v", res)
	}
}

func TestIntegrationOutboxReplicasClaimDisjointRows(t *testing.T) {
	e := newEnv(t)
	for range 150 {
		e.checkpoint(e.tenant, 1, 1)
	}
	var mu sync.Mutex
	seen := map[uuid.UUID]int{}
	var wg sync.WaitGroup
	for range 3 {
		wg.Go(func() {
			rows, err := e.db.ClaimOutbox(e.ctx, time.Now().Add(time.Second), OutboxBatchSize, time.Minute)
			if err != nil {
				t.Error(err)
				return
			}
			mu.Lock()
			for _, r := range rows {
				seen[r.ID]++
			}
			mu.Unlock()
		})
	}
	wg.Wait()
	if len(seen) != 150 {
		t.Fatalf("claimed %d distinct rows of 150", len(seen))
	}
	for id, n := range seen {
		if n != 1 {
			t.Fatalf("row %s claimed %d times", id, n)
		}
	}
}

func TestIntegrationBudgets(t *testing.T) {
	e := newEnv(t, func(v map[string]string) { v["MCPA_AUTHORIZE_CACHE_TTL"] = "200ms" })
	id := e.resource("budget")
	key, _ := e.callKey("owner", id)

	// A stream in progress when the budget is exhausted finishes.
	stall := make(chan struct{})
	e.up.setStall(stall)
	inflight := make(chan callResult, 1)
	go func() { inflight <- e.call(e.srv.URL, id, key, "/v1/chat/completions", chatBody(true), nil) }()
	<-e.up.stalls
	mustDo(e, "owner", e.rpc.SetResourceBudget, &mcpav1.SetResourceBudgetRequest{ResourceId: id, LimitUsd: "0"})
	e.up.setStall(nil)
	rej := e.call(e.srv.URL, id, key, "/v1/chat/completions", chatBody(false), nil)
	if rej.status != http.StatusTooManyRequests || gjson.GetBytes(rej.body, "error.type").String() != "budget_exhausted" {
		t.Fatalf("resource budget: %d %s", rej.status, rej.body)
	}
	if rec := e.waitFinal(rej.callID); rec.Status != mcpav1.CallStatus_CALL_STATUS_REJECTED_BUDGET || rec.ResponseBytes <= 0 {
		t.Fatalf("rejected record %+v", rec)
	}
	close(stall)
	done := <-inflight
	if done.status != http.StatusOK || !strings.Contains(string(done.body), "done") {
		t.Fatalf("in-progress stream: %d %s", done.status, done.body)
	}
	if rec := e.waitFinal(done.callID); rec.Status != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED {
		t.Fatalf("stream record %+v", rec)
	}

	// The tenant budget blocks too.
	mustDo(e, "owner", e.rpc.SetResourceBudget, &mcpav1.SetResourceBudgetRequest{ResourceId: id, LimitUsd: ""})
	mustDo(e, "owner", e.rpc.SetTenantBudget, &mcpav1.SetTenantBudgetRequest{TenantId: e.tenant, LimitUsd: "0"})
	if rej := e.call(e.srv.URL, id, key, "/v1/chat/completions", chatBody(false), nil); rej.status != http.StatusTooManyRequests ||
		!strings.Contains(gjson.GetBytes(rej.body, "error.message").String(), "tenant") {
		t.Fatalf("tenant budget: %d %s", rej.status, rej.body)
	}
	mustDo(e, "owner", e.rpc.SetTenantBudget, &mcpav1.SetTenantBudgetRequest{TenantId: e.tenant, LimitUsd: ""})
	if ok := e.call(e.srv.URL, id, key, "/v1/chat/completions", chatBody(false), nil); ok.status != http.StatusOK {
		t.Fatalf("without limits: %d %s", ok.status, ok.body)
	}

	// Measured spend: a large response exceeds a small positive limit.
	e.up.padding.Store(40_000)
	big := e.call(e.srv.URL, id, key, "/v1/chat/completions", chatBody(false), nil)
	e.waitFinal(big.callID)
	e.up.padding.Store(0)
	mustDo(e, "owner", e.rpc.SetResourceBudget, &mcpav1.SetResourceBudgetRequest{ResourceId: id, LimitUsd: "0.000001"})
	time.Sleep(150 * time.Millisecond) // spend cache
	if rej := e.call(e.srv.URL, id, key, "/v1/chat/completions", chatBody(false), nil); rej.status != http.StatusTooManyRequests {
		t.Fatalf("measured spend: %d %s", rej.status, rej.body)
	}
	budget := mustDo(e, "owner", e.rpc.GetResourceBudget, &mcpav1.GetResourceBudgetRequest{ResourceId: id}).GetBudget()
	if !budget.GetExhausted() || budget.GetSpentUsd() == "0" {
		t.Fatalf("budget %+v", budget)
	}

	// A new billing period resets spend.
	e.dc.set(func(f *fakeDelcore) { f.period.Start = time.Now().UTC() })
	time.Sleep(300 * time.Millisecond) // period and spend caches
	if ok := e.call(e.srv.URL, id, key, "/v1/chat/completions", chatBody(false), nil); ok.status != http.StatusOK {
		t.Fatalf("new period: %d %s", ok.status, ok.body)
	}
	if got := metricValue(t, e, "mcpa_budget_rejections_total", "scope", "resource"); got != 2 {
		t.Fatalf("resource budget rejections %v", got)
	}
	if got := metricValue(t, e, "mcpa_budget_rejections_total", "scope", "tenant"); got != 1 {
		t.Fatalf("tenant budget rejections %v", got)
	}
}

// metricValue reads one counter sample from the server registry.
func metricValue(t *testing.T, e *itEnv, name, label, value string) float64 {
	t.Helper()
	families, err := e.app.Metrics.Registry.Gather()
	if err != nil {
		t.Fatal(err)
	}
	for _, f := range families {
		if f.GetName() != name {
			continue
		}
		for _, m := range f.GetMetric() {
			for _, l := range m.GetLabel() {
				if l.GetName() == label && l.GetValue() == value {
					return m.GetCounter().GetValue()
				}
			}
		}
	}
	return 0
}

func TestIntegrationKeyInvalidation(t *testing.T) {
	e := newEnv(t, func(v map[string]string) { v["MCPA_AUTHORIZE_CACHE_TTL"] = "200ms" })
	id := e.resource("keys")
	other := e.resource("other")
	user := mcpav1.PrincipalKind_PRINCIPAL_KIND_USER
	mustDo(e, "owner", e.rpc.PutAclEntry, &mcpav1.PutAclEntryRequest{ResourceId: id, PrincipalKind: user, PrincipalId: e.users["invoker"], Role: mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE})
	key, keyID := e.callKey("invoker", id)
	status := func(k, rid string) int {
		return e.call(e.srv.URL, rid, k, "/v1/chat/completions", chatBody(false), nil).status
	}
	if got := status(key, id); got != http.StatusOK {
		t.Fatalf("valid key: %d", got)
	}
	if got := status(key, other); got != http.StatusUnauthorized {
		t.Fatalf("cross-resource key: %d", got)
	}

	// Issuer role loss.
	records := e.scalar(`SELECT count(*) FROM call_records`)
	mustDo(e, "owner", e.rpc.RemoveAclEntry, &mcpav1.RemoveAclEntryRequest{ResourceId: id, PrincipalKind: user, PrincipalId: e.users["invoker"]})
	if got := status(key, id); got != http.StatusUnauthorized {
		t.Fatalf("after role loss: %d", got)
	}
	if e.scalar(`SELECT count(*) FROM call_records`) != records {
		t.Fatal("an invalid-key call was recorded")
	}

	// Group roles resolve through the snapshot.
	groupID := uuid.NewString()
	mustDo(e, "owner", e.rpc.PutAclEntry, &mcpav1.PutAclEntryRequest{ResourceId: id, PrincipalKind: mcpav1.PrincipalKind_PRINCIPAL_KIND_GROUP, PrincipalId: groupID, Role: mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE})
	snapshot := func(members ...string) {
		if err := e.db.InTx(e.ctx, func(tx pgx.Tx) error {
			return store.ReplaceGroupSnapshot(e.ctx, tx, e.tenant, []store.Group{{ID: groupID, Name: "Group", UserIDs: members}}, time.Now())
		}); err != nil {
			t.Fatal(err)
		}
	}
	snapshot(e.users["invoker"])
	if got := status(key, id); got != http.StatusOK {
		t.Fatalf("group role: %d", got)
	}
	snapshot()
	if got := status(key, id); got != http.StatusUnauthorized {
		t.Fatalf("after leaving the group: %d", got)
	}
	mustDo(e, "owner", e.rpc.PutAclEntry, &mcpav1.PutAclEntryRequest{ResourceId: id, PrincipalKind: user, PrincipalId: e.users["invoker"], Role: mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE})

	// The issuer leaves the tenant: Authorize(invoke_models, paid) is denied
	// with a permission reason once the cached allowed result expires.
	e.dc.set(func(f *fakeDelcore) {
		f.denied[e.tenant+"/"+e.users["invoker"]+"/invoke_models"] = []delcorev1.BlockReason{delcorev1.BlockReason_BLOCK_REASON_PERMISSION}
	})
	time.Sleep(250 * time.Millisecond)
	if got := status(key, id); got != http.StatusUnauthorized {
		t.Fatalf("after leaving the tenant: %d", got)
	}
	e.dc.set(func(f *fakeDelcore) { delete(f.denied, e.tenant+"/"+e.users["invoker"]+"/invoke_models") })
	// Inactive tenants cannot call; Delcore errors after cache expiry fail
	// closed.
	e.dc.set(func(f *fakeDelcore) { f.states[e.tenant] = delcorev1.TenantState_TENANT_STATE_BLOCKED })
	time.Sleep(250 * time.Millisecond)
	if got := status(key, id); got != http.StatusForbidden {
		t.Fatalf("blocked tenant: %d", got)
	}
	e.dc.set(func(f *fakeDelcore) { delete(f.states, e.tenant); f.down = true })
	time.Sleep(250 * time.Millisecond)
	if got := status(key, id); got != http.StatusServiceUnavailable {
		t.Fatalf("delcore down: %d", got)
	}
	e.dc.set(func(f *fakeDelcore) { f.down = false })
	if got := status(key, id); got != http.StatusOK {
		t.Fatalf("recovered: %d", got)
	}
	var lastUsed *time.Time
	if err := e.db.Pool.QueryRow(e.ctx, `SELECT last_used_time FROM call_keys WHERE id = $1`, keyID).Scan(&lastUsed); err != nil || lastUsed == nil {
		t.Fatalf("last_used_time %v %v", lastUsed, err)
	}

	// Revocation.
	mustDo(e, "invoker", e.rpc.RevokeCallKey, &mcpav1.RevokeCallKeyRequest{ResourceId: id, CallKeyId: keyID})
	if got := status(key, id); got != http.StatusUnauthorized {
		t.Fatalf("revoked: %d", got)
	}
	// Expiry.
	expiring, expiringID := e.callKey("invoker", id)
	if _, err := e.db.Pool.Exec(e.ctx, `UPDATE call_keys SET expire_time = now() - interval '1 second' WHERE id = $1`, expiringID); err != nil {
		t.Fatal(err)
	}
	if got := status(expiring, id); got != http.StatusUnauthorized {
		t.Fatalf("expired: %d", got)
	}
	// Resource deletion.
	ownerKey, _ := e.callKey("owner", id)
	if got := status(ownerKey, id); got != http.StatusOK {
		t.Fatalf("owner key: %d", got)
	}
	mustDo(e, "owner", e.rpc.DeleteResource, &mcpav1.DeleteResourceRequest{ResourceId: id, ConfirmName: "keys"})
	if got := status(ownerKey, id); got != http.StatusUnauthorized {
		t.Fatalf("deleted resource: %d", got)
	}
	// Removing a credential takes effect for the next call.
	otherKey, _ := e.callKey("owner", other)
	creds := mustDo(e, "owner", e.rpc.ListCredentials, &mcpav1.ListCredentialsRequest{ResourceId: other}).GetCredentials()
	mustDo(e, "owner", e.rpc.RemoveCredential, &mcpav1.RemoveCredentialRequest{ResourceId: other, CredentialId: creds[0].GetId()})
	if res := e.call(e.srv.URL, other, otherKey, "/v1/chat/completions", chatBody(false), nil); res.status != http.StatusBadRequest ||
		gjson.GetBytes(res.body, "error.type").String() != "no_credential" {
		t.Fatalf("removed credential: %d %s", res.status, res.body)
	}
}

func TestIntegrationTenantCleanupOnlyAfterDeleted(t *testing.T) {
	e := newEnv(t)
	id := e.resource("doomed")
	key, _ := e.callKey("owner", id)
	if res := e.call(e.srv.URL, id, key, "/v1/chat/completions", chatBody(false), nil); res.status != http.StatusOK {
		t.Fatalf("call %d", res.status)
	} else {
		e.waitFinal(res.callID)
	}
	mustDo(e, "owner", e.rpc.SetTenantBudget, &mcpav1.SetTenantBudgetRequest{TenantId: e.tenant, LimitUsd: "10"})
	survivor := uuid.NewString()
	e.checkpoint(survivor, 3, 3)

	counts := func(tenant string) []int64 {
		var out []int64
		for _, table := range []string{"resources", "call_records", "usage_spend", "usage_daily", "usage_outbox", "audit_events", "tenant_budgets", "credentials", "call_keys"} {
			out = append(out, e.scalar(`SELECT count(*) FROM `+table+` WHERE tenant_id = $1`, tenant))
		}
		return out
	}
	before := counts(e.tenant)
	for _, setup := range []func(*fakeDelcore){
		func(f *fakeDelcore) {},
		func(f *fakeDelcore) { f.states[e.tenant] = delcorev1.TenantState_TENANT_STATE_DELETING },
		func(f *fakeDelcore) { f.states[e.tenant] = delcorev1.TenantState_TENANT_STATE_UNKNOWN },
		func(f *fakeDelcore) { f.down = true },
	} {
		e.dc.set(setup)
		deleted, err := e.dp.LifecycleCycle(e.ctx)
		if err != nil || len(deleted) != 0 {
			t.Fatalf("lifecycle deleted %v (%v)", deleted, err)
		}
		if got := counts(e.tenant); fmt.Sprint(got) != fmt.Sprint(before) {
			t.Fatalf("data changed without DELETED: %v -> %v", before, got)
		}
		e.dc.set(func(f *fakeDelcore) { f.down = false })
	}
	e.dc.set(func(f *fakeDelcore) { f.finalized[e.tenant] = true })
	deleted, err := e.dp.LifecycleCycle(e.ctx)
	if err != nil || len(deleted) != 1 || deleted[0] != e.tenant {
		t.Fatalf("lifecycle deleted %v (%v)", deleted, err)
	}
	for i, n := range counts(e.tenant) {
		if n != 0 {
			t.Fatalf("table %d keeps %d rows of the deleted tenant", i, n)
		}
	}
	if e.scalar(`SELECT count(*) FROM resource_acl`) != 0 || e.scalar(`SELECT count(*) FROM usage_outbox WHERE tenant_id = $1`, survivor) != 1 {
		t.Fatal("cleanup touched the wrong rows")
	}
}

func TestIntegrationRetentionPruning(t *testing.T) {
	e := newEnv(t)
	exec := func(sql string, args ...any) {
		t.Helper()
		if _, err := e.db.Pool.Exec(e.ctx, sql, args...); err != nil {
			t.Fatalf("%s: %v", sql, err)
		}
	}
	old := e.checkpoint(e.tenant, 1, 1)
	fresh := e.checkpoint(e.tenant, 1, 1)
	exec(`UPDATE call_records SET start_time = now() - interval '31 days', expire_time = now() - interval '1 day' WHERE id = $1`, old.CallID)
	exec(`UPDATE usage_daily SET day = (now() - interval '14 months')::date WHERE resource_id = $1`, old.ResourceID)
	exec(`UPDATE usage_daily SET day = (now() - interval '12 months')::date WHERE resource_id = $1`, fresh.ResourceID)
	exec(`INSERT INTO audit_events (id, tenant_id, action, actor_kind, occur_time) VALUES ($1, $2, 1, 1, now() - interval '14 months'), ($3, $2, 1, 1, now() - interval '1 month')`,
		store.NewID(), e.tenant, store.NewID())
	exec(`INSERT INTO idempotency_records (operation, principal_id, request_key, fingerprint, result_id, expire_time) VALUES ('x', 'u', $1, '\x00', $1, now() - interval '1 second')`, store.NewID())

	// A tombstone without retained records is removed; one with records stays.
	empty := e.resource("empty")
	kept := e.resource("kept")
	key, _ := e.callKey("owner", kept)
	res := e.call(e.srv.URL, kept, key, "/v1/chat/completions", chatBody(false), nil)
	e.waitFinal(res.callID)
	for _, r := range []struct{ id, name string }{{empty, "empty"}, {kept, "kept"}} {
		mustDo(e, "owner", e.rpc.DeleteResource, &mcpav1.DeleteResourceRequest{ResourceId: r.id, ConfirmName: r.name})
	}
	exec(`UPDATE audit_events SET occur_time = now() - interval '14 months' WHERE resource_id = $1`, empty)
	// Provider OAuth flows end one day before pruning.
	live := e.resource("flows")
	for _, f := range []struct {
		state   int
		updated string
		expire  string
	}{{3, "2 days", "1 hour"}, {1, "1 hour", "-1 hour"}, {2, "1 hour", "1 hour"}, {1, "3 days", "2 days"}} {
		exec(`INSERT INTO provider_oauth_flows (id, resource_id, tenant_id, provider, kind, state, name, expire_time, created_by_user_id,
				update_time, key_id, wrapped_key, key_nonce, nonce, ciphertext)
			VALUES ($1, $2, $3, 4, 1, $4, 'f', now() - $6::interval, 'u', now() - $5::interval, 'k1', '\x00', '\x00', '\x00', '\x00')`,
			store.NewID(), live, e.tenant, f.state, f.updated, f.expire)
	}

	if err := e.dp.RetentionCycle(e.ctx); err != nil {
		t.Fatal(err)
	}
	checks := []struct {
		sql  string
		args []any
		want int64
	}{
		{`SELECT count(*) FROM call_records WHERE id = $1`, []any{old.CallID}, 0},
		{`SELECT count(*) FROM call_records WHERE id = $1`, []any{fresh.CallID}, 1},
		{`SELECT count(*) FROM usage_daily WHERE resource_id = $1`, []any{old.ResourceID}, 0},
		{`SELECT count(*) FROM usage_daily WHERE resource_id = $1`, []any{fresh.ResourceID}, 1},
		{`SELECT count(*) FROM audit_events WHERE resource_id IS NULL AND tenant_id = $1`, []any{e.tenant}, 1},
		{`SELECT count(*) FROM idempotency_records WHERE operation = 'x'`, nil, 0},
		{`SELECT count(*) FROM resources WHERE id = $1`, []any{empty}, 0},
		{`SELECT count(*) FROM resources WHERE id = $1`, []any{kept}, 1},
		{`SELECT count(*) FROM resource_acl WHERE resource_id = $1`, []any{kept}, 1},
		{`SELECT count(*) FROM provider_oauth_flows WHERE resource_id = $1`, []any{live}, 2},
	}
	for _, c := range checks {
		if got := e.scalar(c.sql, c.args...); got != c.want {
			t.Fatalf("%s %v = %d, want %d", c.sql, c.args, got, c.want)
		}
	}
	// The kept tombstone is still readable by its manager.
	usage := mustDo(e, "owner", e.rpc.GetUsage, &mcpav1.GetUsageRequest{TenantId: e.tenant, Filter: &mcpav1.CallFilter{ResourceIds: []string{kept}}})
	if usage.GetSummary().GetCallCount() != 1 {
		t.Fatalf("tombstone usage %+v", usage.GetSummary())
	}
}

func TestIntegrationProviderOAuth(t *testing.T) {
	e := newEnv(t)
	id := e.resource("oauth")
	var tokenRequests atomic.Int64
	var kimiPending atomic.Bool
	kimiPending.Store(true)
	e.setOAuthHandler(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case "/claude/token":
			tokenRequests.Add(1)
			if gjson.GetBytes(body, "code").String() != "good-code" {
				w.WriteHeader(400)
				_, _ = io.WriteString(w, `{"error":"invalid_grant"}`)
				return
			}
			_, _ = io.WriteString(w, `{"access_token":"sk-ant-oat-CANARY","refresh_token":"sk-ant-ort-CANARY","expires_in":3600,"account":{"email_address":"dev@example.com"}}`)
		case "/kimi/device":
			_, _ = io.WriteString(w, `{"device_code":"kimi-device-CANARY","user_code":"KIMI-1","verification_uri":"https://www.kimi.com/device","expires_in":900,"interval":5}`)
		case "/kimi/token":
			if kimiPending.Load() {
				_, _ = io.WriteString(w, `{"error":"authorization_pending"}`)
				return
			}
			_, _ = io.WriteString(w, `{"access_token":"kimi-access-CANARY","refresh_token":"kimi-refresh-CANARY","token_type":"Bearer","expires_in":3600}`)
		default:
			http.NotFound(w, r)
		}
	})

	start := func(p mcpav1.Provider) *mcpav1.ProviderOAuthFlow {
		return mustDo(e, "owner", e.rpc.StartProviderOAuth, &mcpav1.StartProviderOAuthRequest{ResourceId: id, Provider: p, Name: p.String()}).GetFlow()
	}
	flow := start(mcpav1.Provider_PROVIDER_CLAUDE)
	if flow.GetKind() != mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_AUTHORIZATION_CODE || flow.GetState() != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_PENDING {
		t.Fatalf("flow %+v", flow)
	}
	authURL, _ := url.Parse(flow.GetAuthorizationUrl())
	state := authURL.Query().Get("state")
	if state == "" || authURL.Query().Get("redirect_uri") != "http://localhost:54545/callback" {
		t.Fatalf("authorization URL %s", authURL)
	}
	// A URL of another flow is refused and leaves the flow pending.
	if _, err := do(e, "owner", e.rpc.CompleteProviderOAuth, &mcpav1.CompleteProviderOAuthRequest{ResourceId: id, FlowId: flow.GetId(),
		CallbackUrl: "http://localhost:54545/callback?code=good-code&state=wrong"}); connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("wrong state: %v", err)
	}
	if tokenRequests.Load() != 0 {
		t.Fatal("a wrong state reached the provider")
	}
	done := mustDo(e, "owner", e.rpc.CompleteProviderOAuth, &mcpav1.CompleteProviderOAuthRequest{ResourceId: id, FlowId: flow.GetId(),
		CallbackUrl: "http://localhost:54545/callback?code=good-code%23" + state + "&state=" + state}).GetFlow()
	if done.GetState() != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_COMPLETED || done.GetCredentialId() == "" || done.GetAuthorizationUrl() != "" {
		t.Fatalf("completed flow %+v", done)
	}
	again := mustDo(e, "owner", e.rpc.CompleteProviderOAuth, &mcpav1.CompleteProviderOAuthRequest{ResourceId: id, FlowId: flow.GetId(),
		CallbackUrl: "http://localhost:54545/callback?code=good-code&state=" + state}).GetFlow()
	if again.GetCredentialId() != done.GetCredentialId() || tokenRequests.Load() != 1 {
		t.Fatalf("replayed completion %+v after %d token requests", again, tokenRequests.Load())
	}
	var found bool
	for _, c := range mustDo(e, "owner", e.rpc.ListCredentials, &mcpav1.ListCredentialsRequest{ResourceId: id}).GetCredentials() {
		if c.GetId() == done.GetCredentialId() {
			found = c.GetKind() == mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH && c.GetAccountLabel() == "dev@example.com" && c.GetProvider() == mcpav1.Provider_PROVIDER_CLAUDE
		}
	}
	if !found {
		t.Fatal("the OAuth credential is not listed")
	}
	audit := mustDo(e, "owner", e.rpc.ListAuditEvents, &mcpav1.ListAuditEventsRequest{ResourceId: id}).GetEvents()
	if audit[0].GetAction() != mcpav1.AuditAction_AUDIT_ACTION_CREDENTIAL_REGISTERED || audit[0].GetTargetId() != done.GetCredentialId() {
		t.Fatalf("audit %+v", audit[0])
	}
	// The engine serves the new credential's models immediately.
	key, _ := e.callKey("owner", id)
	req, _ := http.NewRequest(http.MethodGet, e.srv.URL+"/r/"+id+"/v1/models", nil)
	req.Header.Set("Authorization", "Bearer "+key)
	req.Header.Set("Anthropic-Version", "2023-06-01")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	models, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != 200 || !strings.Contains(string(models), "claude") {
		t.Fatalf("models %d %s", resp.StatusCode, models)
	}

	// A flow past its expiry cannot complete.
	late := start(mcpav1.Provider_PROVIDER_CLAUDE)
	e.clock.advance(11 * time.Minute)
	lateURL, _ := url.Parse(late.GetAuthorizationUrl())
	expired := mustDo(e, "owner", e.rpc.CompleteProviderOAuth, &mcpav1.CompleteProviderOAuthRequest{ResourceId: id, FlowId: late.GetId(),
		CallbackUrl: "http://localhost:54545/callback?code=good-code&state=" + lateURL.Query().Get("state")}).GetFlow()
	if expired.GetState() != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_EXPIRED || tokenRequests.Load() != 1 {
		t.Fatalf("late flow %+v", expired)
	}

	// Kimi device flow: polls follow the provider interval.
	device := start(mcpav1.Provider_PROVIDER_KIMI)
	if device.GetKind() != mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_DEVICE_CODE || device.GetUserCode() != "KIMI-1" || device.GetVerificationUri() == "" {
		t.Fatalf("device flow %+v", device)
	}
	get := func() *mcpav1.ProviderOAuthFlow {
		return mustDo(e, "owner", e.rpc.GetProviderOAuthFlow, &mcpav1.GetProviderOAuthFlowRequest{ResourceId: id, FlowId: device.GetId()}).GetFlow()
	}
	if f := get(); f.GetState() != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_PENDING {
		t.Fatalf("early poll %+v", f)
	}
	e.clock.advance(6 * time.Second)
	if f := get(); f.GetState() != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_PENDING {
		t.Fatalf("pending poll %+v", f)
	}
	kimiPending.Store(false)
	e.clock.advance(6 * time.Second)
	completed := get()
	if completed.GetState() != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_COMPLETED || completed.GetCredentialId() == "" {
		t.Fatalf("device completion %+v", completed)
	}
	if _, err := do(e, "owner", e.rpc.CompleteProviderOAuth, &mcpav1.CompleteProviderOAuthRequest{ResourceId: id, FlowId: device.GetId(),
		CallbackUrl: "http://localhost/?code=x"}); connect.CodeOf(err) != connect.CodeFailedPrecondition {
		t.Fatalf("completing a device flow: %v", err)
	}

	// Tokens, verifiers and device codes are stored encrypted only.
	verifierFree := []string{"sk-ant-oat-CANARY", "sk-ant-ort-CANARY", "kimi-device-CANARY", "kimi-access-CANARY", state}
	assertNoSecretsOrBodies(t, e, verifierFree...)
}

// Budget spend of a 31-day billing period that starts partway through a UTC
// day stays counted after the call records of its first day expire.
func TestIntegrationBudgetSpendOutlivesCallRecords(t *testing.T) {
	e := newEnv(t)
	now := time.Now().UTC()
	// The period started 31 days minus two hours ago, partway through a UTC
	// day, and lasts 31 days.
	start := now.Add(-31*24*time.Hour + 2*time.Hour).Truncate(time.Hour).Add(17 * time.Minute)
	end := start.Add(31 * 24 * time.Hour)
	resourceID, keyID := store.NewID(), store.NewID()
	apply := func(startTime time.Time, bytes int64) store.Checkpoint {
		t.Helper()
		c := store.Checkpoint{
			CallID: store.NewID(), TenantID: e.tenant, ResourceID: resourceID, CallKeyID: keyID,
			Provider: mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE, Model: "m", Format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_OPENAI,
			StartTime: startTime, Seq: 1, DeltaRequestBytes: bytes, DeltaResponseBytes: 0,
			RequestBytes: bytes, ResponseBytes: 0, OutboxID: store.NewID(), RequestKey: store.NewID(),
		}
		if _, err := e.db.ApplyCheckpoint(e.ctx, c); err != nil {
			t.Fatal(err)
		}
		return c
	}
	first := apply(start.Add(time.Minute), 1000)              // partial first day
	middle := apply(start.Add(3*24*time.Hour), 20)            // fully covered day
	apply(start.Add(-time.Minute), 300)                       // previous period
	second := apply(first.StartTime.Add(time.Microsecond), 4) // a second call on the first day
	// A later checkpoint of the first call adds to its spend entry.
	next := first
	next.Seq, next.DeltaRequestBytes, next.RequestBytes = 2, 5, first.RequestBytes+5
	next.Final, next.Status = true, mcpav1.CallStatus_CALL_STATUS_SUCCEEDED
	next.OutboxID, next.RequestKey = store.NewID(), store.NewID()
	if _, err := e.db.ApplyCheckpoint(e.ctx, next); err != nil {
		t.Fatal(err)
	}
	want := int64(1000 + 5 + 20 + 4)
	billed := func() (int64, int64) {
		t.Helper()
		r, err := e.dp.Usage.ResourceBilledBytes(e.ctx, resourceID, start, end)
		if err != nil {
			t.Fatal(err)
		}
		tenant, err := e.dp.Usage.TenantBilledBytes(e.ctx, e.tenant, start, end)
		if err != nil {
			t.Fatal(err)
		}
		return r, tenant
	}
	// The resource row is required by ResourceBilledBytes.
	if _, err := e.db.Pool.Exec(e.ctx, `INSERT INTO resources (id, tenant_id, name, created_by_user_id) VALUES ($1, $2, 'spend', 'u')`,
		resourceID, e.tenant); err != nil {
		t.Fatal(err)
	}
	if r, tenant := billed(); r != want || tenant != want {
		t.Fatalf("billed before retention: resource %d tenant %d, want %d", r, tenant, want)
	}

	if err := e.dp.RetentionCycle(e.ctx); err != nil {
		t.Fatal(err)
	}
	if n := e.scalar(`SELECT count(*) FROM call_records WHERE id = ANY($1)`, []uuid.UUID{first.CallID, second.CallID}); n != 0 {
		t.Fatalf("%d first-day call records survived retention", n)
	}
	if n := e.scalar(`SELECT count(*) FROM call_records WHERE id = $1`, middle.CallID); n != 1 {
		t.Fatal("the call record of a later day was pruned")
	}
	if r, tenant := billed(); r != want || tenant != want {
		t.Fatalf("billed after call record retention: resource %d tenant %d, want %d", r, tenant, want)
	}

	// Spend entries expire 35 days after the call start.
	old := apply(now.Add(-36*24*time.Hour), 9)
	if err := e.dp.RetentionCycle(e.ctx); err != nil {
		t.Fatal(err)
	}
	if n := e.scalar(`SELECT count(*) FROM usage_spend WHERE start_time = $1`, old.StartTime); n != 0 {
		t.Fatal("an expired spend entry survived retention")
	}
	if n := e.scalar(`SELECT count(*) FROM usage_spend WHERE tenant_id = $1`, e.tenant); n != 4 {
		t.Fatalf("retained spend entries %d, want 4", n)
	}
}

// Migration 0003 backfills spend entries from retained call records.
func TestIntegrationUsageSpendMigrationBackfill(t *testing.T) {
	dbURL := testDatabase(t)
	db, err := store.Open(t.Context(), dbURL, 10*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(db.Close)
	ms, err := store.Migrations()
	if err != nil {
		t.Fatal(err)
	}
	ctx := t.Context()
	exec := func(sql string, args ...any) {
		t.Helper()
		if _, err := db.Pool.Exec(ctx, sql, args...); err != nil {
			t.Fatalf("%s: %v", sql, err)
		}
	}
	// Bring the schema to version 2 as an earlier binary did.
	exec(`CREATE TABLE mcpa_schema_migrations (version integer PRIMARY KEY, name text NOT NULL, applied_at timestamptz NOT NULL DEFAULT now())`)
	for _, m := range ms[:2] {
		exec(m.SQL)
		exec(`INSERT INTO mcpa_schema_migrations (version, name) VALUES ($1, $2)`, m.Version, m.Name)
	}
	resourceID, at := store.NewID(), time.Now().UTC().Add(-time.Hour).Truncate(time.Microsecond)
	for i, bytes := range [][2]int64{{10, 5}, {1, 2}, {0, 0}} {
		startTime := at
		if i == 2 {
			startTime = at.Add(time.Second)
		}
		exec(`INSERT INTO call_records (id, tenant_id, resource_id, call_key_id, format, request_bytes, response_bytes, checkpoint_seq,
				start_time, expire_time) VALUES ($1, 't1', $2, $3, 1, $4, $5, 1, $6, $6::timestamptz + interval '30 days')`,
			store.NewID(), resourceID, store.NewID(), bytes[0], bytes[1], startTime)
	}
	applied, err := db.Migrate(ctx)
	if err != nil || len(applied) != len(ms)-2 || applied[0] != 3 {
		t.Fatalf("migrate applied %v: %v", applied, err)
	}
	var n, sum int64
	if err := db.Pool.QueryRow(ctx, `SELECT count(*), COALESCE(sum(bytes), 0) FROM usage_spend WHERE tenant_id = 't1' AND resource_id = $1`,
		resourceID).Scan(&n, &sum); err != nil {
		t.Fatal(err)
	}
	if n != 1 || sum != 18 {
		t.Fatalf("backfilled %d entries with %d bytes, want 1 entry with 18 bytes", n, sum)
	}
	if err := db.CheckSchema(ctx); err != nil {
		t.Fatal(err)
	}
}

// credentialNotifications listens on the credential change channel with its
// own connection. Other tests share the database, so callers filter by
// payload.
type credentialNotifications struct {
	t        *testing.T
	payloads chan string
}

func listenCredentialChanges(e *itEnv) *credentialNotifications {
	e.t.Helper()
	conn, err := pgx.Connect(e.ctx, e.dbURL)
	if err != nil {
		e.t.Fatal(err)
	}
	if _, err := conn.Exec(e.ctx, "LISTEN "+pgx.Identifier{credentialChannel}.Sanitize()); err != nil {
		e.t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(e.ctx)
	done := make(chan struct{})
	n := &credentialNotifications{t: e.t, payloads: make(chan string, 256)}
	go func() {
		defer close(done)
		for {
			msg, err := conn.WaitForNotification(ctx)
			if err != nil {
				return
			}
			n.payloads <- msg.Payload
		}
	}()
	e.t.Cleanup(func() {
		cancel()
		<-done
		_ = conn.Close(context.Background())
	})
	return n
}

// received waits for payload and reports whether it arrived within wait.
func (n *credentialNotifications) received(payload string, wait time.Duration) bool {
	timeout := time.After(wait)
	for {
		select {
		case got := <-n.payloads:
			if got == payload {
				return true
			}
		case <-timeout:
			return false
		}
	}
}

// Credential changes reach other replicas through notifications queued in
// the transaction of the change: a committed change always notifies, a
// rolled-back change never does, a failed notification rolls the change
// back, and a replica that loaded a removed credential stops selecting it
// without reconnecting its listener.
func TestIntegrationCredentialChangesNotifyOtherReplicas(t *testing.T) {
	e := newEnv(t)
	notes := listenCredentialChanges(e)
	payload := func(resourceID string) string { return e.dp.Sync.instance + ":" + resourceID }

	id := e.resource("sync")
	if !notes.received(payload(id), 5*time.Second) {
		t.Fatal("registering a credential sent no notification")
	}
	rid := uuid.MustParse(id)
	creds := mustDo(e, "owner", e.rpc.ListCredentials, &mcpav1.ListCredentialsRequest{ResourceId: id}).GetCredentials()
	credID := uuid.MustParse(creds[0].GetId())

	// Replica B shares the database and listens for changes.
	appName := "mcpa-replica-" + store.NewID().String()
	dbB, err := store.Open(e.ctx, e.dbURL+"&application_name="+appName, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(dbB.Close)
	cfgB := e.dp.cfg
	cfgB.Store, cfgB.Metrics = dbB, nil
	replica, err := New(cfgB)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = replica.Close() })
	listenCtx, stopListen := context.WithCancel(e.ctx)
	listening := make(chan struct{})
	go func() {
		defer close(listening)
		_ = replica.Sync.Listen(listenCtx)
	}()
	t.Cleanup(func() {
		stopListen()
		<-listening
	})
	deadline := time.Now().Add(5 * time.Second)
	for e.scalar(`SELECT count(*) FROM pg_stat_activity WHERE application_name = $1 AND query LIKE 'LISTEN %'`, appName) == 0 {
		if time.Now().After(deadline) {
			t.Fatal("replica B does not listen for credential changes")
		}
		time.Sleep(20 * time.Millisecond)
	}
	srvB := httptest.NewServer(replica.Engine)
	t.Cleanup(srvB.Close)
	key, _ := e.callKey("owner", id)
	// The listener is connected, so B only drops this runtime when a
	// notification arrives.
	if res := e.call(srvB.URL, id, key, "/v1/chat/completions", chatBody(false), nil); res.status != http.StatusOK {
		t.Fatalf("replica B call: %d %s", res.status, res.body)
	}

	// A rolled-back change sends no notification.
	errAbort := errors.New("abort")
	if err := e.db.InTx(e.ctx, func(tx pgx.Tx) error {
		if _, err := store.DeleteCredential(e.ctx, tx, rid, credID); err != nil {
			return err
		}
		if err := e.dp.Sync.CredentialsChanging(e.ctx, tx, rid); err != nil {
			return err
		}
		return errAbort
	}); !errors.Is(err, errAbort) {
		t.Fatalf("rolled-back change: %v", err)
	}
	if notes.received(payload(id), 300*time.Millisecond) {
		t.Fatal("a rolled-back change sent a notification")
	}

	// A notification that cannot be queued rolls the change back.
	oversized := &CredentialSync{db: e.db, log: e.dp.log, engine: e.dp.Engine, instance: strings.Repeat("x", 8000)}
	if err := e.db.InTx(e.ctx, func(tx pgx.Tx) error {
		if _, err := store.DeleteCredential(e.ctx, tx, rid, credID); err != nil {
			return err
		}
		return oversized.CredentialsChanging(e.ctx, tx, rid)
	}); err == nil {
		t.Fatal("an oversized notification was queued")
	}
	if n := e.scalar(`SELECT count(*) FROM credentials WHERE id = $1`, credID); n != 1 {
		t.Fatal("a change whose notification failed was committed")
	}
	if res := e.call(srvB.URL, id, key, "/v1/chat/completions", chatBody(false), nil); res.status != http.StatusOK {
		t.Fatalf("replica B call after rolled-back changes: %d %s", res.status, res.body)
	}

	// Removal on replica A reaches replica B.
	mustDo(e, "owner", e.rpc.RemoveCredential, &mcpav1.RemoveCredentialRequest{ResourceId: id, CredentialId: credID.String()})
	if !notes.received(payload(id), 5*time.Second) {
		t.Fatal("removing a credential sent no notification")
	}
	deadline = time.Now().Add(5 * time.Second)
	for {
		res := e.call(srvB.URL, id, key, "/v1/chat/completions", chatBody(false), nil)
		if res.status == http.StatusBadRequest && gjson.GetBytes(res.body, "error.type").String() == "no_credential" {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("replica B still serves the removed credential: %d %s", res.status, res.body)
		}
		time.Sleep(20 * time.Millisecond)
	}

	// Resource deletion and tenant deletion notify as well.
	deleted := e.resource("deleted")
	if !notes.received(payload(deleted), 5*time.Second) {
		t.Fatal("registering a credential sent no notification")
	}
	mustDo(e, "owner", e.rpc.DeleteResource, &mcpav1.DeleteResourceRequest{ResourceId: deleted, ConfirmName: "deleted"})
	if !notes.received(payload(deleted), 5*time.Second) {
		t.Fatal("deleting a resource sent no notification")
	}
	if err := e.dp.deleteTenant(e.ctx, e.tenant); err != nil {
		t.Fatal(err)
	}
	if !notes.received(payload(id), 5*time.Second) {
		t.Fatal("deleting the tenant sent no notification")
	}
}
