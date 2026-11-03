// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package dataplane

import (
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/tidwall/gjson"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

const geminiRefreshedAccess = "refreshed-access-CANARY"

// serveFakeGoogle answers the connection flow of a Gemini CLI account. The
// access token it issues expires after one second.
func serveFakeGoogle(e *itEnv) {
	e.setOAuthHandler(func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case "/google/token":
			_, _ = io.WriteString(w, `{"access_token":"initial-access-CANARY","refresh_token":"refresh-token-CANARY","token_type":"Bearer","expires_in":1}`)
		case "/google/userinfo":
			_, _ = io.WriteString(w, `{"email":"gemini@example.com"}`)
		case "/codeassist/v1internal:loadCodeAssist":
			_, _ = io.WriteString(w, `{"cloudaicompanionProject":"proj-1","allowedTiers":[{"id":"free-tier","isDefault":true}]}`)
		case "/codeassist/v1internal:onboardUser":
			_, _ = io.WriteString(w, `{"done":true,"response":{"cloudaicompanionProject":{"id":"proj-1"}}}`)
		default:
			http.NotFound(w, r)
		}
	})
}

// An expired Gemini CLI token is refreshed once, the new payload is stored
// encrypted, the other replica never refreshes again, and the refresh
// traffic is not billed as call bytes. The credential is connected through
// the real OAuth broker flow against fake Google endpoints; the model and
// token endpoints are fakes behind the provider transport.
func TestIntegrationGeminiCLIRefreshPersistsAndSharesAcrossReplicas(t *testing.T) {
	e := newEnv(t, func(vars map[string]string) {
		vars["MCPA_GEMINI_CLI_OAUTH_CLIENT_ID"] = "operator-gemini-client"
		vars["MCPA_GEMINI_CLI_OAUTH_CLIENT_SECRET"] = "operator-gemini-secret"
	})
	id := e.resource("gemini-refresh")

	// The fake Google endpoints issue an access token that expires after one
	// second, so the first call finds it expired.
	serveFakeGoogle(e)
	flow := mustDo(e, "owner", e.rpc.StartProviderOAuth, &mcpav1.StartProviderOAuthRequest{ResourceId: id, Provider: mcpav1.Provider_PROVIDER_GEMINI_CLI, Name: "gemini"}).GetFlow()
	authURL, _ := url.Parse(flow.GetAuthorizationUrl())
	state := authURL.Query().Get("state")
	done := mustDo(e, "owner", e.rpc.CompleteProviderOAuth, &mcpav1.CompleteProviderOAuthRequest{ResourceId: id, FlowId: flow.GetId(),
		CallbackUrl: "http://localhost:8085/oauth2callback?code=good-code&state=" + state}).GetFlow()
	if done.GetState() != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_COMPLETED || done.GetCredentialId() == "" {
		t.Fatalf("connection flow %+v", done)
	}

	// Fake provider endpoints reached through the provider transport.
	var tokenRequests atomic.Int64
	e.up.handle("/token", func(w http.ResponseWriter, r *http.Request) {
		tokenRequests.Add(1)
		body, _ := io.ReadAll(r.Body)
		form, _ := url.ParseQuery(string(body))
		w.Header().Set("Content-Type", "application/json")
		if form.Get("grant_type") != "refresh_token" || form.Get("refresh_token") != "refresh-token-CANARY" ||
			form.Get("client_id") != "operator-gemini-client" || form.Get("client_secret") != "operator-gemini-secret" {
			w.WriteHeader(http.StatusBadRequest)
			_, _ = io.WriteString(w, `{"error":"invalid_grant"}`)
			return
		}
		_, _ = io.WriteString(w, `{"access_token":"`+geminiRefreshedAccess+`","token_type":"Bearer","expires_in":3600}`)
	})
	var modelAuths []string
	e.up.handle("/v1internal:generateContent", func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.Copy(io.Discard, r.Body)
		e.up.mu.Lock()
		modelAuths = append(modelAuths, r.Header.Get("Authorization"))
		e.up.mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"hello from gemini cli"}]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}}}`)
	})

	key, _ := e.callKey("owner", id)
	body := `{"model":"gemini-2.5-pro","messages":[{"role":"user","content":"` + promptCanary + `"}]}`
	notes := listenCredentialChanges(e)

	// Three calls on replica A: one token request, equal billed request
	// bytes (the token request is not part of any call's bytes).
	var billed []int64
	for i := 0; i < 3; i++ {
		res := e.call(e.srv.URL, id, key, "/v1/chat/completions", body, nil)
		if res.status != http.StatusOK || !strings.Contains(string(res.body), "hello from gemini cli") {
			t.Fatalf("call %d on replica A: %d %s", i, res.status, res.body)
		}
		record := e.waitFinal(res.callID)
		if record.Status != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED || record.Provider != mcpav1.Provider_PROVIDER_GEMINI_CLI {
			t.Fatalf("call %d record %+v", i, record)
		}
		billed = append(billed, record.RequestBytes)
	}
	if billed[0] <= 0 || billed[0] != billed[1] || billed[1] != billed[2] {
		t.Fatalf("billed request bytes differ across calls: %v", billed)
	}
	if got := tokenRequests.Load(); got != 1 {
		t.Fatalf("%d token requests for three calls, want 1", got)
	}

	// The refreshed payload is stored encrypted and decrypts to the new
	// expiry and tokens, and is not readable in the database.
	var secret []byte
	deadline := time.Now().Add(10 * time.Second)
	for {
		creds, err := (&Credentials{db: e.db, keys: e.dp.cfg.Keys, log: e.dp.log}).LoadResourceCredentials(e.ctx, id)
		if err != nil {
			t.Fatal(err)
		}
		for _, cred := range creds {
			if cred.ID == done.GetCredentialId() && gjson.GetBytes(cred.Secret, "token.access_token").String() == geminiRefreshedAccess {
				secret = cred.Secret
			}
		}
		if secret != nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("the refreshed payload was not stored")
		}
		time.Sleep(20 * time.Millisecond)
	}
	expiry, err := time.Parse(time.RFC3339, gjson.GetBytes(secret, "token.expiry").String())
	if err != nil || time.Until(expiry) < 50*time.Minute {
		t.Fatalf("stored expiry %q (%v) did not advance", gjson.GetBytes(secret, "token.expiry").String(), err)
	}
	if gjson.GetBytes(secret, "token.refresh_token").String() != "refresh-token-CANARY" || gjson.GetBytes(secret, "project_id").String() != "proj-1" ||
		gjson.GetBytes(secret, "token.client_id").String() != "operator-gemini-client" {
		t.Fatalf("stored payload lost fields: %s", secret)
	}
	assertNoSecretsOrBodies(t, e, geminiRefreshedAccess, "refresh-token-CANARY", "initial-access-CANARY")

	// The refresh queued the credential-change notification in the storing
	// transaction. Replica B loads the stored payload and makes no token
	// request for its own calls. (Both replicas share this process and its
	// SDK model registry, so B starts after the refresh instead of reacting
	// to the notification; a replica that reacts to it reloads the same
	// stored payload.)
	if !notes.received(e.dp.Sync.instance+":"+id, 5*time.Second) {
		t.Fatal("the refreshed token sent no notification")
	}
	_, srvB := startReplica(e)
	for i := 0; i < 2; i++ {
		res := e.call(srvB.URL, id, key, "/v1/chat/completions", body, nil)
		if res.status != http.StatusOK {
			t.Fatalf("call %d on replica B: %d %s", i, res.status, res.body)
		}
		if record := e.waitFinal(res.callID); record.RequestBytes != billed[0] {
			t.Fatalf("replica B billed %d request bytes, want %d", record.RequestBytes, billed[0])
		}
	}
	if got := tokenRequests.Load(); got != 1 {
		t.Fatalf("%d token requests after replica B calls, want 1", got)
	}
	e.up.mu.Lock()
	defer e.up.mu.Unlock()
	if len(modelAuths) != 5 {
		t.Fatalf("model endpoint saw %d requests, want 5", len(modelAuths))
	}
	for i, auth := range modelAuths {
		if auth != "Bearer "+geminiRefreshedAccess {
			t.Fatalf("model request %d used %q", i, auth)
		}
	}
}

// A revoked refresh token is a provider authentication error with the
// provider's error code, and the stored payload is left unchanged.
func TestIntegrationGeminiCLIRevokedRefreshTokenIsProviderAuthError(t *testing.T) {
	e := newEnv(t, func(vars map[string]string) {
		vars["MCPA_GEMINI_CLI_OAUTH_CLIENT_ID"] = "operator-gemini-client"
		vars["MCPA_GEMINI_CLI_OAUTH_CLIENT_SECRET"] = "operator-gemini-secret"
	})
	id := e.resource("gemini-revoked")
	serveFakeGoogle(e)
	flow := mustDo(e, "owner", e.rpc.StartProviderOAuth, &mcpav1.StartProviderOAuthRequest{ResourceId: id, Provider: mcpav1.Provider_PROVIDER_GEMINI_CLI, Name: "gemini"}).GetFlow()
	authURL, _ := url.Parse(flow.GetAuthorizationUrl())
	mustDo(e, "owner", e.rpc.CompleteProviderOAuth, &mcpav1.CompleteProviderOAuthRequest{ResourceId: id, FlowId: flow.GetId(),
		CallbackUrl: "http://localhost:8085/oauth2callback?code=good-code&state=" + authURL.Query().Get("state")})

	e.up.handle("/token", func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusBadRequest)
		_, _ = io.WriteString(w, `{"error":"invalid_grant","error_description":"Token has been expired or revoked."}`)
	})
	key, _ := e.callKey("owner", id)
	body := `{"model":"gemini-2.5-pro","messages":[{"role":"user","content":"hi"}]}`
	res := e.call(e.srv.URL, id, key, "/v1/chat/completions", body, nil)
	if res.status != http.StatusUnauthorized || !strings.Contains(string(res.body), "invalid_grant") {
		t.Fatalf("revoked token: %d %s", res.status, res.body)
	}
	record := e.waitFinal(res.callID)
	if record.Status != mcpav1.CallStatus_CALL_STATUS_PROVIDER_AUTH_ERROR || record.RequestBytes != 0 {
		t.Fatalf("record %+v", record)
	}
}
