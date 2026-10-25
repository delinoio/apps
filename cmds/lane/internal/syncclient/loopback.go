// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"errors"
	"net"
	"net/http"
	"time"
)

var ErrCallbackPort = errors.New("browser callback port is unavailable")

const LoopbackRedirectURI = "http://127.0.0.1:46327/auth/callback"

// ApproveLoopback supports a standalone CLI without an installed custom-scheme
// handler. The caller displays or opens the approval URL through show. The
// listener binds before approval and never accepts non-loopback traffic.
func (a Auth) ApproveLoopback(ctx context.Context, s Store, endpoint, expectedAccountID string, confirmEpochChange bool, show func(Approval) error) (Credential, error) {
	listener, e := net.Listen("tcp4", "127.0.0.1:46327")
	if e != nil {
		return Credential{}, ErrCallbackPort
	}
	return a.approveListener(ctx, s, endpoint, expectedAccountID, confirmEpochChange, show, listener, LoopbackRedirectURI)
}
func (a Auth) approveListener(ctx context.Context, s Store, endpoint, expectedAccountID string, confirmed bool, show func(Approval) error, listener net.Listener, redirectURI string) (Credential, error) {
	defer listener.Close()
	ctx, cancel := context.WithTimeout(ctx, 10*time.Minute)
	defer cancel()
	approval, e := a.BeginStore(ctx, s, endpoint, redirectURI, expectedAccountID, confirmed)
	if e != nil {
		return Credential{}, e
	}
	type result struct {
		credential Credential
		err        error
	}
	done := make(chan result, 1)
	mux := http.NewServeMux()
	mux.HandleFunc("/auth/callback", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
		if r.Method != http.MethodGet || r.Host != listener.Addr().String() || r.URL.Query().Get("state") != approval.ChallengeID {
			http.Error(w, "Invalid approval callback.", http.StatusBadRequest)
			return
		}
		credential, err := a.CompleteStore(ctx, s, redirectURI+"?"+r.URL.RawQuery)
		if err != nil {
			http.Error(w, "Approval failed. Return to Lane and retry.", http.StatusBadRequest)
		} else {
			_, _ = w.Write([]byte("Lane approval completed. You can close this tab."))
		}
		select {
		case done <- result{credential, err}:
		default:
		}
	})
	server := &http.Server{Handler: mux, ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 10 * time.Second, WriteTimeout: 30 * time.Second, IdleTimeout: 5 * time.Second, MaxHeaderBytes: 16 << 10}
	served := make(chan struct{})
	go func() { defer close(served); _ = server.Serve(listener) }()
	defer func() {
		shutdown, stop := context.WithTimeout(context.Background(), 3*time.Second)
		defer stop()
		_ = server.Shutdown(shutdown)
		_ = server.Close()
		<-served
	}()
	if show == nil {
		return Credential{}, errors.New("browser approval URL display is required")
	}
	if e = show(approval); e != nil {
		return Credential{}, e
	}
	select {
	case result := <-done:
		return result.credential, result.err
	case <-ctx.Done():
		return Credential{}, ctx.Err()
	}
}

// ApproveDeletionLoopback receives only reauthentication. It returns a public
// challenge ID and never dispatches deletion or exposes the private proof.
// StoreDeletionChallenge must have been called with the BeginDataDeletion result.
func (a Auth) ApproveDeletionLoopback(ctx context.Context, s Store, c Credential, challengeID, authorizationURL string, show func(string) error) (string, error) {
	if c.RedirectURI != LoopbackRedirectURI || !validUUID(challengeID) {
		return "", ErrApproval
	}
	pending, e := a.readDeletion(deletionKey(s.SpaceID(), challengeID))
	if e != nil || !pending.matches(s, c, pending.Mode, challengeID) {
		return "", ErrApproval
	}
	listener, e := net.Listen("tcp4", "127.0.0.1:46327")
	if e != nil {
		return "", ErrCallbackPort
	}
	defer listener.Close()
	ctx, cancel := context.WithTimeout(ctx, time.Until(pending.ExpiresAt))
	defer cancel()
	type result struct {
		id  string
		err error
	}
	done := make(chan result, 1)
	mux := http.NewServeMux()
	mux.HandleFunc("/auth/callback", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
		if r.Method != http.MethodGet || r.Host != "127.0.0.1:46327" || r.URL.Query().Get("reauth_challenge") != challengeID {
			http.Error(w, "Invalid reauthentication callback.", http.StatusBadRequest)
			return
		}
		id, e := a.CompleteDeletion(ctx, s, c, LoopbackRedirectURI+"?"+r.URL.RawQuery)
		if e != nil {
			http.Error(w, "Reauthentication failed. Return to Lane and retry.", http.StatusBadRequest)
		} else {
			_, _ = w.Write([]byte("Reauthentication completed. Return to Lane to confirm deletion."))
		}
		select {
		case done <- result{id, e}:
		default:
		}
	})
	server := &http.Server{Handler: mux, ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 10 * time.Second, WriteTimeout: 30 * time.Second, IdleTimeout: 5 * time.Second, MaxHeaderBytes: 16 << 10}
	served := make(chan struct{})
	go func() { defer close(served); _ = server.Serve(listener) }()
	defer func() {
		shutdown, stop := context.WithTimeout(context.Background(), 3*time.Second)
		defer stop()
		_ = server.Shutdown(shutdown)
		_ = server.Close()
		<-served
	}()
	if show == nil {
		return "", errors.New("browser reauthentication URL display is required")
	}
	if e = show(authorizationURL); e != nil {
		return "", e
	}
	select {
	case result := <-done:
		return result.id, result.err
	case <-ctx.Done():
		return "", ctx.Err()
	}
}
