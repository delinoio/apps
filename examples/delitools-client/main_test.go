// SPDX-License-Identifier: Apache-2.0

package main

import (
	"bytes"
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"github.com/delinoio/apps/protos/delitools/v1/delitoolsv1connect"
)

type fakeDelitools struct {
	delitoolsv1connect.UnimplementedDelitoolsServiceHandler
	token string
}

func (f *fakeDelitools) authorized(h interface{ Get(string) string }) error {
	if h.Get("Authorization") != "Bearer "+f.token {
		return connect.NewError(connect.CodeUnauthenticated, errors.New("authentication failed"))
	}
	return nil
}

func (f *fakeDelitools) Translate(_ context.Context, r *connect.Request[pb.TranslateRequest]) (*connect.Response[pb.TranslateResponse], error) {
	if err := f.authorized(r.Header()); err != nil {
		return nil, err
	}
	translations := make([]string, len(r.Msg.GetTexts()))
	for i, text := range r.Msg.GetTexts() {
		translations[i] = strings.ToUpper(text) + "@" + r.Msg.GetTargetLanguage()
	}
	return connect.NewResponse(&pb.TranslateResponse{RequestId: "req-1", Translations: translations}), nil
}

func (f *fakeDelitools) Extract(_ context.Context, r *connect.Request[pb.ExtractRequest]) (*connect.Response[pb.ExtractResponse], error) {
	if err := f.authorized(r.Header()); err != nil {
		return nil, err
	}
	err := connect.NewError(connect.CodeFailedPrecondition, errors.New("the text does not contain the information required by the schema"))
	detail, _ := connect.NewErrorDetail(&pb.ErrorDetail{Reason: pb.ErrorReason_ERROR_REASON_EXTRACTION_FAILED, RequestId: "req-2"})
	err.AddDetail(detail)
	return nil, err
}

func writeRequest(t *testing.T, content string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "request.json")
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatal(err)
	}
	return path
}

func environment(url, token string) func(string) string {
	return func(key string) string {
		switch key {
		case "DELITOOLS_URL":
			return url
		case "DELITOOLS_TOKEN":
			return token
		}
		return ""
	}
}

func TestTranslatePreservesOrderAndSendsBearerToken(t *testing.T) {
	path, handler := delitoolsv1connect.NewDelitoolsServiceHandler(&fakeDelitools{token: "dlk_fixture"})
	server := httptest.NewServer(newMux(path, handler))
	defer server.Close()
	request := writeRequest(t, `{"tenantId":"t","texts":["one","two"],"targetLanguage":"fr","model":"m"}`)
	var out bytes.Buffer
	if err := run(context.Background(), []string{"translate", request}, environment(server.URL, "dlk_fixture"), &out, server.Client()); err != nil {
		t.Fatal(err)
	}
	if got := out.String(); got != "ONE@fr\nTWO@fr\n" {
		t.Fatalf("unexpected output %q", got)
	}
}

func TestMissingCredentialsAndUsageAreRejectedBeforeAnyRequest(t *testing.T) {
	request := writeRequest(t, `{}`)
	var out bytes.Buffer
	for _, args := range [][]string{{}, {"translate"}, {"other", request}} {
		if err := run(context.Background(), args, environment("http://127.0.0.1:1", "token"), &out, nil); err == nil {
			t.Fatalf("expected a usage error for %v", args)
		}
	}
	if err := run(context.Background(), []string{"translate", request}, environment("", ""), &out, nil); err == nil {
		t.Fatal("expected a configuration error")
	}
}

func TestErrorsReportTheStableReasonWithoutContent(t *testing.T) {
	path, handler := delitoolsv1connect.NewDelitoolsServiceHandler(&fakeDelitools{token: "token"})
	server := httptest.NewServer(newMux(path, handler))
	defer server.Close()
	request := writeRequest(t, `{"tenantId":"t","text":"private text","jsonSchema":"{}"}`)
	err := run(context.Background(), []string{"extract", request}, environment(server.URL, "token"), &bytes.Buffer{}, server.Client())
	if err == nil {
		t.Fatal("expected an error")
	}
	message := describe(err)
	if !strings.Contains(message, "ERROR_REASON_EXTRACTION_FAILED") || !strings.Contains(message, "req-2") {
		t.Fatalf("unexpected message %q", message)
	}
	if strings.Contains(message, "private text") {
		t.Fatal("request content must not be printed")
	}
	unauthenticated := run(context.Background(), []string{"extract", request}, environment(server.URL, "wrong"), &bytes.Buffer{}, server.Client())
	if got := describe(unauthenticated); !strings.Contains(got, "unauthenticated") {
		t.Fatalf("unexpected message %q", got)
	}
}

func newMux(path string, handler http.Handler) *http.ServeMux {
	mux := http.NewServeMux()
	mux.Handle(path, handler)
	return mux
}
