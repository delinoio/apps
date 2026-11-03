// SPDX-License-Identifier: FSL-1.1-ALv2

package server_test

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"connectrpc.com/connect"
	"google.golang.org/protobuf/proto"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/protos/mcpa/v1/mcpav1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/config"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/server"
	"github.com/delinoio/apps/servers/mcpa-api/internal/web"
)

const registerAPIKey = "/" + mcpav1connect.McpaServiceName + "/RegisterApiKeyCredential"

// stubService records the calls that reach the service after authentication.
type stubService struct {
	mcpav1connect.UnimplementedMcpaServiceHandler
	listed []string
	reg    int
}

func (s *stubService) ListResources(_ context.Context, req *connect.Request[mcpav1.ListResourcesRequest]) (*connect.Response[mcpav1.ListResourcesResponse], error) {
	s.listed = append(s.listed, req.Msg.GetTenantId())
	return connect.NewResponse(&mcpav1.ListResourcesResponse{}), nil
}

func (s *stubService) RegisterApiKeyCredential(context.Context, *connect.Request[mcpav1.RegisterApiKeyCredentialRequest]) (*connect.Response[mcpav1.RegisterApiKeyCredentialResponse], error) {
	s.reg++
	return connect.NewResponse(&mcpav1.RegisterApiKeyCredentialResponse{}), nil
}

type personalKeys struct{}

func (personalKeys) AuthorizeAPIKey(context.Context, string, delcore.Capability, bool) (delcore.KeyDecision, error) {
	return delcore.KeyDecision{OwnerKind: delcorev1.ApiKeyOwnerKind_API_KEY_OWNER_KIND_PERSONAL, UserID: "u1", TenantID: "t1"}, nil
}

func newCodecHandler(t *testing.T, svc mcpav1connect.McpaServiceHandler, logs *bytes.Buffer) http.Handler {
	t.Helper()
	log := obs.NewLogger(logs, config.Logging{Format: config.LogFormatJSON})
	authn := auth.NewAuthenticator(auth.AuthenticatorConfig{WebOrigin: webOrigin, Keys: personalKeys{}})
	return server.New(server.Config{
		Service: svc, Authenticator: authn,
		Web:     web.New(web.Config{Authenticator: authn, Log: slog.New(slog.NewTextHandler(io.Discard, nil))}),
		Metrics: obs.NewMetrics(), Log: log, WebOrigin: webOrigin,
	})
}

func post(h http.Handler, path, contentType string, body []byte, header map[string]string) *httptest.ResponseRecorder {
	req := httptest.NewRequest(http.MethodPost, path, bytes.NewReader(body))
	req.Header.Set("Content-Type", contentType)
	for k, v := range header {
		req.Header.Set(k, v)
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec
}

// connectError is the Connect-protocol JSON error body.
type connectError struct {
	Code    string `json:"code"`
	Message string `json:"message"`
	Details []struct {
		Type  string `json:"type"`
		Value string `json:"value"`
	} `json:"details"`
}

func decodeError(t *testing.T, rec *httptest.ResponseRecorder) (connectError, *mcpav1.ErrorDetail) {
	t.Helper()
	var ce connectError
	if err := json.Unmarshal(rec.Body.Bytes(), &ce); err != nil {
		t.Fatalf("body is not a Connect error: %v: %s", err, rec.Body.String())
	}
	if len(ce.Details) != 1 || ce.Details[0].Type != "mcpa.v1.ErrorDetail" {
		t.Fatalf("details = %+v", ce.Details)
	}
	raw, err := base64.RawStdEncoding.DecodeString(strings.TrimRight(ce.Details[0].Value, "="))
	if err != nil {
		t.Fatal(err)
	}
	var detail mcpav1.ErrorDetail
	if err := proto.Unmarshal(raw, &detail); err != nil {
		t.Fatal(err)
	}
	return ce, &detail
}

func TestMalformedRequestBodyIsTypedAndContentFree(t *testing.T) {
	const secret = "sk-test-SECRET"
	jsonCases := map[string]string{
		"unquoted value":   `{"api_key": ` + secret + `}`,
		"wrong type":       `{"api_key": 12345678901234}`,
		"wrong enum type":  `{"provider": ["` + secret + `"]}`,
		"truncated":        `{"api_key": "` + secret,
		"not an object":    `"` + secret + `"`,
		"trailing garbage": `{"api_key": "x"} ` + secret,
		"empty":            ``,
	}
	for _, contentType := range []string{"application/json", "application/json; charset=utf-8", "application/proto"} {
		cases := jsonCases
		if contentType == "application/proto" {
			// Wire type 7 does not exist; the text also tries to smuggle a secret.
			cases = map[string]string{"garbage": "\xff\xff\xff\xff" + secret, "truncated field": "\x22\x7f" + secret}
		}
		for name, body := range cases {
			t.Run(contentType+"/"+name, func(t *testing.T) {
				var logs bytes.Buffer
				svc := &stubService{}
				h := newCodecHandler(t, svc, &logs)
				// A valid-looking credential is present so that only the body
				// can explain the rejection.
				rec := post(h, registerAPIKey, contentType, []byte(body), map[string]string{"Authorization": "Bearer dlk_valid"})
				if rec.Code != http.StatusBadRequest {
					t.Fatalf("status = %d: %s", rec.Code, rec.Body.String())
				}
				if svc.reg != 0 {
					t.Fatal("the service saw a request that failed to decode")
				}
				ce, detail := decodeError(t, rec)
				if ce.Code != "invalid_argument" || ce.Message != "request body is not a valid message" {
					t.Fatalf("error = %+v", ce)
				}
				ids := rec.Header().Values(server.RequestIDHeader)
				if len(ids) != 1 || ids[0] == "" || detail.GetTraceId() != ids[0] {
					t.Fatalf("trace_id %q, %s %q", detail.GetTraceId(), server.RequestIDHeader, ids)
				}
				if detail.GetReason() != mcpav1.ErrorReason_ERROR_REASON_INVALID_INPUT {
					t.Fatalf("reason = %v", detail.GetReason())
				}
				for _, out := range []string{rec.Body.String(), logs.String()} {
					for _, leak := range []string{secret, "12345678901234", "unmarshal", "proto:", "syntax error"} {
						if strings.Contains(out, leak) {
							t.Fatalf("output leaks %q: %s", leak, out)
						}
					}
				}
				lines := strings.Split(strings.TrimSpace(logs.String()), "\n")
				if len(lines) != 1 || !strings.Contains(lines[0], `"management call rejected"`) ||
					strings.Count(lines[0], `"trace_id"`) != 1 || !strings.Contains(lines[0], `"trace_id":"`+ids[0]+`"`) {
					t.Fatalf("log lines = %q", lines)
				}
			})
		}
	}
}

// TestMalformedBodyFailsBeforeAuthentication keeps the order connect-go had:
// an undecodable body is rejected without consulting credentials.
func TestMalformedBodyFailsBeforeAuthentication(t *testing.T) {
	var logs bytes.Buffer
	h := newCodecHandler(t, &stubService{}, &logs)
	rec := post(h, registerAPIKey, "application/json", []byte(`{"api_key": nope}`), nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("status = %d", rec.Code)
	}
	if _, detail := decodeError(t, rec); detail.GetReason() != mcpav1.ErrorReason_ERROR_REASON_INVALID_INPUT {
		t.Fatalf("reason = %v", detail.GetReason())
	}
}

func TestValidRequestsAreUnchanged(t *testing.T) {
	var logs bytes.Buffer
	svc := &stubService{}
	h := newCodecHandler(t, svc, &logs)

	// Valid unauthenticated request: typed unauthenticated error, as before.
	rec := post(h, getViewer, "application/json", []byte(`{}`), nil)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("unauthenticated status = %d", rec.Code)
	}
	if _, detail := decodeError(t, rec); detail.GetReason() != mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED {
		t.Fatalf("reason = %v", detail.GetReason())
	}

	// Valid authenticated JSON, JSON with unknown fields and binary requests
	// reach the service with their content intact.
	auth := map[string]string{"Authorization": "Bearer dlk_valid"}
	listResources := "/" + mcpav1connect.McpaServiceName + "/ListResources"
	if rec := post(h, listResources, "application/json", []byte(`{"tenantId": "t-json", "futureField": 1}`), auth); rec.Code != http.StatusOK {
		t.Fatalf("json status = %d: %s", rec.Code, rec.Body.String())
	}
	bin, err := proto.Marshal(&mcpav1.ListResourcesRequest{TenantId: "t-bin"})
	if err != nil {
		t.Fatal(err)
	}
	rec = post(h, listResources, "application/proto", bin, auth)
	if rec.Code != http.StatusOK {
		t.Fatalf("proto status = %d: %s", rec.Code, rec.Body.String())
	}
	var resp mcpav1.ListResourcesResponse
	if err := proto.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("response is not binary proto: %v", err)
	}
	if got := strings.Join(svc.listed, ","); got != "t-json,t-bin" {
		t.Fatalf("service saw %q", got)
	}
}
