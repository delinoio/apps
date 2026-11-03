// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"runtime"
	"strings"
	"testing"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/google/uuid"
)

// redirectTransport sends every request to the fake provider, whatever host
// the SDK executor built the URL with. The SDK Kimi executor hardcodes
// api.kimi.com.
type redirectTransport struct {
	target string
	base   http.RoundTripper
}

func (r redirectTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	out := req.Clone(req.Context())
	out.URL.Scheme = "http"
	out.URL.Host = r.target
	out.Host = r.target
	return r.base.RoundTrip(out)
}

func kimiChatUpstream(t *testing.T) *upstream {
	return newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(openAIChatResponse("kimi-test", "hello")))
	})
}

func kimiOAuthCredential(t *testing.T, resourceID, deviceID string) Credential {
	payload := map[string]any{
		"type":         "kimi",
		"access_token": "kimi-access-token",
		"expired":      "2099-01-01T00:00:00Z",
	}
	if deviceID != "" {
		payload["device_id"] = deviceID
	}
	secret, err := json.Marshal(payload)
	if err != nil {
		t.Fatal(err)
	}
	return Credential{
		ID:         uuid.Must(uuid.NewV7()).String(),
		ResourceID: resourceID,
		Provider:   mcpav1.Provider_PROVIDER_KIMI,
		Kind:       mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH,
		Secret:     secret,
		Models:     []string{"kimi-test"},
	}
}

func assertNoHostDetails(t *testing.T, header http.Header) {
	t.Helper()
	hostname, _ := os.Hostname()
	platform := runtime.GOOS + " " + runtime.GOARCH
	for name, values := range header {
		if !strings.HasPrefix(strings.ToLower(name), "x-msh-device") {
			continue
		}
		for _, value := range values {
			if (hostname != "" && strings.Contains(value, hostname)) || strings.Contains(value, platform) ||
				strings.Contains(value, runtime.GOOS) || strings.Contains(value, runtime.GOARCH) {
				t.Fatalf("%s carries host details: %q", name, value)
			}
		}
	}
}

// Kimi provider requests identify mcpa and the credential, never the server
// host, and the billed request bytes stay those of the rewritten request.
func TestKimiRequestsDoNotDiscloseHostDetails(t *testing.T) {
	body := `{"model":"kimi-test","messages":[{"role":"user","content":"hi"}]}`
	for _, tc := range []struct {
		name       string
		credential func(resourceID string) Credential
		deviceID   func(Credential) string
	}{
		{
			name: "oauth with stored device id",
			credential: func(resourceID string) Credential {
				return kimiOAuthCredential(t, resourceID, "stored-device-id")
			},
			deviceID: func(Credential) string { return "stored-device-id" },
		},
		{
			name: "oauth without stored device id",
			credential: func(resourceID string) Credential {
				return kimiOAuthCredential(t, resourceID, "")
			},
			deviceID: func(c Credential) string {
				return uuid.NewSHA1(kimiDeviceNamespace, []byte(c.ID)).String()
			},
		},
		{
			name: "api key",
			credential: func(resourceID string) Credential {
				return apiKeyCredential(resourceID, mcpav1.Provider_PROVIDER_KIMI, "", "sk-kimi-test", "kimi-test")
			},
			deviceID: func(c Credential) string {
				return uuid.NewSHA1(kimiDeviceNamespace, []byte(c.ID)).String()
			},
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			up := kimiChatUpstream(t)
			h := newHarness(t, func(o *Options) {
				o.BaseTransport = redirectTransport{target: strings.TrimPrefix(up.URL(), "http://"), base: http.DefaultTransport}
			})
			resourceID := newResourceID()
			key := h.addKey(resourceID)
			cred := tc.credential(resourceID)
			h.addCredential(cred)
			res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, body, nil)
			if res.resp.StatusCode != http.StatusOK {
				t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
			}
			headers := up.headersSeen()
			if len(headers) != 1 {
				t.Fatalf("upstream saw %d requests", len(headers))
			}
			got := headers[0]
			if got.Get("X-Msh-Device-Name") != "Managed CPA" || got.Get("X-Msh-Device-Model") != "mcpa-api" {
				t.Fatalf("device name %q model %q", got.Get("X-Msh-Device-Name"), got.Get("X-Msh-Device-Model"))
			}
			if want := tc.deviceID(cred); got.Get("X-Msh-Device-Id") != want {
				t.Fatalf("device id %q, want %q", got.Get("X-Msh-Device-Id"), want)
			}
			assertNoHostDetails(t, got)
			// The provider behaviour is otherwise the SDK's.
			if got.Get("Authorization") == "" || got.Get("X-Msh-Platform") == "" {
				t.Fatalf("SDK headers were dropped: %v", got)
			}
			record := h.sink.waitFinal(t, res.callID)
			if record.Status != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED {
				t.Fatalf("record %+v", record)
			}
			// Billed request bytes equal the bytes the provider received,
			// which include the rewritten headers.
			req, _ := h.sink.sums(res.callID)
			if want := up.read.Load(); req != want {
				t.Fatalf("billed %d request bytes, upstream received %d", req, want)
			}
		})
	}
}

// The device ID of a credential without a stored one is stable across calls
// and differs between credentials, so the provider cannot link tenants
// through a host-level device ID.
func TestKimiDeviceIDIsPerCredentialAndStable(t *testing.T) {
	first := kimiOAuthCredential(t, "r", "")
	second := kimiOAuthCredential(t, "r", "")
	auth := func(c Credential) string {
		built, err := buildAuth(c, time.Now())
		if err != nil {
			t.Fatal(err)
		}
		return kimiDeviceIDFor(built.auth)
	}
	if a, b := auth(first), auth(first); a == "" || a != b {
		t.Fatalf("device ID is not stable: %q %q", a, b)
	}
	if auth(first) == auth(second) {
		t.Fatal("two credentials share a device ID")
	}
	if _, err := uuid.Parse(auth(first)); err != nil {
		t.Fatalf("device ID is not a UUID: %v", err)
	}
}

// Claude-format Kimi calls go through the SDK Claude executor, which sends no
// X-Msh-Device headers and bypasses the metering transport. The rewrite must
// not add any header to requests that do not carry the SDK's device headers,
// and must not touch other providers' requests.
func TestKimiRewriteIsLimitedToKimiRequestsWithDeviceHeaders(t *testing.T) {
	up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(http.StatusNoContent) })
	send := func(call *callState, header map[string]string) http.Header {
		t.Helper()
		before := len(up.headersSeen())
		req, _ := http.NewRequestWithContext(withCall(context.Background(), call), http.MethodPost, up.URL()+"/v1/chat/completions", strings.NewReader("{}"))
		for name, value := range header {
			req.Header.Set(name, value)
		}
		resp, err := (&http.Client{Transport: &meteringTransport{base: http.DefaultTransport}}).Do(req)
		if err != nil {
			t.Fatal(err)
		}
		_ = resp.Body.Close()
		seen := up.headersSeen()
		if len(seen) != before+1 {
			t.Fatalf("upstream saw %d requests", len(seen))
		}
		return seen[len(seen)-1]
	}
	hostHeaders := map[string]string{"X-Msh-Device-Name": "host-1", "X-Msh-Device-Model": "linux amd64", "X-Msh-Device-Id": "host-id"}

	kimi := &callState{id: "kimi"}
	kimi.selected("cred", mcpav1.Provider_PROVIDER_KIMI)
	kimi.setKimiDeviceID("device-1")
	// A Kimi request without the SDK's device headers (Claude format) gains none.
	for name, values := range send(kimi, nil) {
		if strings.HasPrefix(strings.ToLower(name), "x-msh-") {
			t.Fatalf("header %s=%v was added", name, values)
		}
	}
	// A Kimi request with host headers is rewritten.
	if got := send(kimi, hostHeaders); got.Get("X-Msh-Device-Name") != "Managed CPA" || got.Get("X-Msh-Device-Id") != "device-1" {
		t.Fatalf("headers not rewritten: %v", got)
	}
	// Another provider's request is left alone.
	other := &callState{id: "other"}
	other.selected("cred", mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE)
	if got := send(other, hostHeaders); got.Get("X-Msh-Device-Name") != "host-1" {
		t.Fatalf("a non-Kimi request was rewritten: %v", got)
	}
}
