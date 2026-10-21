// SPDX-License-Identifier: Apache-2.0

package main

import (
	"bytes"
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"github.com/delinoio/apps/protos/delitools/v1/delitoolsv1connect"
)

const testRequestID = "019a0000-0000-7000-8000-000000000001"

// fakeDeliTools records calls and returns fixed results. When failExtract is
// set it returns a typed extraction-impossible error.
type fakeDeliTools struct {
	delitoolsv1connect.UnimplementedDeliToolsServiceHandler
	mu          sync.Mutex
	calls       []string
	translate   *pb.TranslateTextArrayRequest
	extract     *pb.ExtractDataFromTextRequest
	failExtract bool
	// delay is how long each call takes before it answers.
	delay time.Duration
}

func (f *fakeDeliTools) authorize(name string, h http.Header) error {
	time.Sleep(f.delay)
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = append(f.calls, name)
	if h.Get("Authorization") != "Bearer test-key" {
		return connect.NewError(connect.CodeUnauthenticated, errors.New("invalid credential"))
	}
	return nil
}

func (f *fakeDeliTools) TranslateTextArray(_ context.Context, r *connect.Request[pb.TranslateTextArrayRequest]) (*connect.Response[pb.TranslateTextArrayResponse], error) {
	if e := f.authorize("TranslateTextArray", r.Header()); e != nil {
		return nil, e
	}
	f.translate = r.Msg
	out := make([]string, len(r.Msg.Texts))
	for i, text := range r.Msg.Texts {
		out[i] = "[" + r.Msg.OutputLanguage + "] " + text
	}
	return connect.NewResponse(&pb.TranslateTextArrayResponse{RequestId: testRequestID, Texts: out}), nil
}

func (f *fakeDeliTools) ExtractDataFromText(_ context.Context, r *connect.Request[pb.ExtractDataFromTextRequest]) (*connect.Response[pb.ExtractDataFromTextResponse], error) {
	if e := f.authorize("ExtractDataFromText", r.Header()); e != nil {
		return nil, e
	}
	f.extract = r.Msg
	if f.failExtract {
		err := connect.NewError(connect.CodeFailedPrecondition, errors.New("required information is missing"))
		detail, _ := connect.NewErrorDetail(&pb.ErrorDetail{Reason: pb.ErrorReason_ERROR_REASON_EXTRACTION_IMPOSSIBLE, RequestId: testRequestID})
		err.AddDetail(detail)
		return nil, err
	}
	return connect.NewResponse(&pb.ExtractDataFromTextResponse{RequestId: testRequestID, JsonData: `{"name":"Ada Lovelace","city":"London"}`}), nil
}

func newServer(t *testing.T, fake *fakeDeliTools) string {
	t.Helper()
	mux := http.NewServeMux()
	mux.Handle(delitoolsv1connect.NewDeliToolsServiceHandler(fake))
	server := httptest.NewServer(mux)
	t.Cleanup(server.Close)
	return server.URL
}

func TestRunTranslatesAndExtracts(t *testing.T) {
	fake := &fakeDeliTools{}
	url := newServer(t, fake)
	o := options{baseURL: url, credential: "test-key", tenantID: "tenant-1", outputLang: "ko", model: "model-a"}
	var out bytes.Buffer
	client := delitoolsv1connect.NewDeliToolsServiceClient(http.DefaultClient, url)
	if e := run(context.Background(), client, o, &out); e != nil {
		t.Fatal(e)
	}
	if strings.Join(fake.calls, ",") != "TranslateTextArray,ExtractDataFromText" {
		t.Fatalf("unexpected calls %v", fake.calls)
	}
	if fake.translate.TenantId != "tenant-1" || fake.translate.OutputLanguage != "ko" || fake.translate.Model != "model-a" || len(fake.translate.Texts) != 2 {
		t.Fatalf("unexpected translate request %v", fake.translate)
	}
	if fake.translate.IdempotencyKey == "" || fake.extract.IdempotencyKey == "" || fake.translate.IdempotencyKey == fake.extract.IdempotencyKey {
		t.Fatal("each call needs its own idempotency key")
	}
	if !strings.Contains(fake.extract.JsonSchema, `"additionalProperties": false`) {
		t.Fatal("schema must use the supported subset")
	}
	text := out.String()
	for _, want := range []string{"Translated 2 texts (request " + testRequestID, "Extracted 39 bytes of JSON data (request " + testRequestID} {
		if !strings.Contains(text, want) {
			t.Errorf("output missing %q:\n%s", want, text)
		}
	}
	// Response content is application data and must never reach the output.
	for _, content := range []string{"[ko] Good morning", "[ko] Where is the station?", "Ada Lovelace", "London"} {
		if strings.Contains(text, content) {
			t.Errorf("output must not contain response content %q:\n%s", content, text)
		}
	}
	if strings.Contains(text, "test-key") {
		t.Fatal("output must not contain the credential")
	}
}

func TestRunReusesCallerIdempotencyKeys(t *testing.T) {
	fake := &fakeDeliTools{}
	url := newServer(t, fake)
	o := options{baseURL: url, credential: "test-key", tenantID: "tenant-1", outputLang: "ko", model: "model-a", translateKey: "translate-retry-key", extractKey: "extract-retry-key"}
	var out bytes.Buffer
	client := delitoolsv1connect.NewDeliToolsServiceClient(http.DefaultClient, url)
	if e := run(context.Background(), client, o, &out); e != nil {
		t.Fatal(e)
	}
	if fake.translate.IdempotencyKey != "translate-retry-key" || fake.extract.IdempotencyKey != "extract-retry-key" {
		t.Fatalf("caller keys must be sent unchanged: %q %q", fake.translate.IdempotencyKey, fake.extract.IdempotencyKey)
	}
	// Generated keys are printed too, so an uncertain run can be repeated.
	fake2 := &fakeDeliTools{}
	url2 := newServer(t, fake2)
	o2 := options{baseURL: url2, credential: "test-key", tenantID: "tenant-1", outputLang: "ko", model: "model-a"}
	var out2 bytes.Buffer
	client2 := delitoolsv1connect.NewDeliToolsServiceClient(http.DefaultClient, url2)
	if e := run(context.Background(), client2, o2, &out2); e != nil {
		t.Fatal(e)
	}
	for _, key := range []string{fake2.translate.IdempotencyKey, fake2.extract.IdempotencyKey} {
		if !strings.Contains(out2.String(), key) {
			t.Errorf("generated key %q must be printed:\n%s", key, out2.String())
		}
	}
}

func TestRunAppliesTheTimeoutToEachCall(t *testing.T) {
	// Each call takes 150 ms and the deadline is 250 ms: together they exceed
	// one shared deadline, but each fits its own.
	url := newServer(t, &fakeDeliTools{delay: 150 * time.Millisecond})
	o := options{baseURL: url, credential: "test-key", tenantID: "tenant-1", outputLang: "ko", model: "model-a", timeout: 250 * time.Millisecond}
	client := delitoolsv1connect.NewDeliToolsServiceClient(http.DefaultClient, url)
	if e := run(context.Background(), client, o, &bytes.Buffer{}); e != nil {
		t.Fatalf("a per-call deadline must not span both calls: %v", e)
	}
	o.timeout = 50 * time.Millisecond
	if e := run(context.Background(), client, o, &bytes.Buffer{}); connect.CodeOf(e) != connect.CodeDeadlineExceeded {
		t.Fatalf("expected deadline_exceeded for a call over its own deadline, got %v", e)
	}
}

func TestRunSelectsOneOperation(t *testing.T) {
	for _, tc := range []struct {
		op   operation
		want string
	}{
		{operationTranslate, "TranslateTextArray"},
		{operationExtract, "ExtractDataFromText"},
		{operationAll, "TranslateTextArray,ExtractDataFromText"},
	} {
		fake := &fakeDeliTools{}
		url := newServer(t, fake)
		o := options{baseURL: url, credential: "test-key", tenantID: "tenant-1", outputLang: "ko", model: "model-a", operation: tc.op}
		client := delitoolsv1connect.NewDeliToolsServiceClient(http.DefaultClient, url)
		if e := run(context.Background(), client, o, &bytes.Buffer{}); e != nil {
			t.Fatal(e)
		}
		if got := strings.Join(fake.calls, ","); got != tc.want {
			t.Errorf("operation %d called %q, want %q", tc.op, got, tc.want)
		}
	}
}

func TestParseOperation(t *testing.T) {
	for value, want := range map[string]operation{"all": operationAll, "translate": operationTranslate, "extract": operationExtract} {
		if got, e := parseOperation(value); e != nil || got != want {
			t.Errorf("parseOperation(%q) = %v, %v", value, got, e)
		}
	}
	if _, e := parseOperation("both"); e == nil {
		t.Fatal("an unknown operation must fail")
	}
	if operationExtract.translates() {
		t.Fatal("the extract operation must not translate, so no model is needed")
	}
}

func TestRunReportsTypedErrorWithoutContent(t *testing.T) {
	url := newServer(t, &fakeDeliTools{failExtract: true})
	o := options{baseURL: url, credential: "test-key", tenantID: "tenant-1", outputLang: "ko", model: "model-a"}
	client := delitoolsv1connect.NewDeliToolsServiceClient(http.DefaultClient, url)
	e := run(context.Background(), client, o, &bytes.Buffer{})
	if e == nil {
		t.Fatal("expected an error")
	}
	got := describe(e)
	want := "code=failed_precondition reason=ERROR_REASON_EXTRACTION_IMPOSSIBLE request_id=" + testRequestID
	if got != want {
		t.Fatalf("describe = %q, want %q", got, want)
	}
}

func TestRunRejectsWrongCredential(t *testing.T) {
	url := newServer(t, &fakeDeliTools{})
	o := options{baseURL: url, credential: "wrong", tenantID: "tenant-1", outputLang: "ko", model: "model-a"}
	client := delitoolsv1connect.NewDeliToolsServiceClient(http.DefaultClient, url)
	if e := run(context.Background(), client, o, &bytes.Buffer{}); connect.CodeOf(e) != connect.CodeUnauthenticated {
		t.Fatalf("expected unauthenticated, got %v", e)
	}
}

func TestOptionsFromEnv(t *testing.T) {
	env := func(values map[string]string) func(string) string {
		return func(k string) string { return values[k] }
	}
	o, e := optionsFromEnv(env(map[string]string{"DELITOOLS_TOKEN": "t", "DELITOOLS_TENANT_ID": "x"}))
	if e != nil || o.credential != "t" || o.baseURL != defaultURL {
		t.Fatalf("unexpected options %v %v", o, e)
	}
	o, _ = optionsFromEnv(env(map[string]string{"DELITOOLS_API_KEY": "k", "DELITOOLS_TOKEN": "t", "DELITOOLS_TENANT_ID": "x", "DELITOOLS_URL": "http://h"}))
	if o.credential != "k" || o.baseURL != "http://h" {
		t.Fatalf("API key and URL must win: %v", o)
	}
	if _, e := optionsFromEnv(env(map[string]string{"DELITOOLS_API_KEY": "k"})); e == nil {
		t.Fatal("missing tenant must fail")
	}
}
