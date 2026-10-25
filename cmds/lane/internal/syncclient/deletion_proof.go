// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"encoding/json"
	"net/url"
	"time"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
)

type deletionChallenge struct {
	SpaceID, DeviceID, Issuer, AccountID, TenantID, SessionID, Endpoint, ChallengeID, RedirectURI string
	Mode                                                                                          lanev1.DataDeletionMode
	ExpiresAt                                                                                     time.Time
	Proof                                                                                         string
	Used                                                                                          bool
}

func deletionKey(space, challenge string) string { return "deletion:" + space + ":" + challenge }

// StoreDeletionChallenge persists a server-issued challenge, never a caller
// supplied proof. The account identity must come from the pinned space's grant.
func (a Auth) StoreDeletionChallenge(ctx context.Context, s Store, c Credential, mode lanev1.DataDeletionMode, challengeID string) error {
	if c.DeviceID != s.DeviceID() || c.AccountID == "" || c.TenantID == "" || c.Issuer == "" || c.SessionID == "" || !validUUID(challengeID) || (mode != lanev1.DataDeletionMode_DATA_DELETION_MODE_DELETE_EVERYWHERE && mode != lanev1.DataDeletionMode_DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL) || redirectURL(c.RedirectURI) != nil {
		return ErrApproval
	}
	key := deletionKey(s.SpaceID(), challengeID)
	unlock, e := a.lockCredential(ctx, key)
	if e != nil {
		return e
	}
	defer unlock()
	// A duplicate Begin response cannot extend proof validity or revive consumption.
	if raw, e := a.vault().Get(key); e == nil {
		var existing deletionChallenge
		if json.Unmarshal([]byte(raw), &existing) != nil || !existing.matches(s, c, mode, challengeID) {
			return ErrApproval
		}
		return nil
	}
	pending := deletionChallenge{SpaceID: s.SpaceID(), DeviceID: c.DeviceID, Issuer: c.Issuer, AccountID: c.AccountID, TenantID: c.TenantID, SessionID: c.SessionID, Endpoint: c.Endpoint, ChallengeID: challengeID, RedirectURI: c.RedirectURI, Mode: mode, ExpiresAt: time.Now().Add(5 * time.Minute)}
	raw, e := json.Marshal(pending)
	if e != nil {
		return e
	}
	return a.vault().Set(key, string(raw))
}
func (p deletionChallenge) matches(s Store, c Credential, mode lanev1.DataDeletionMode, challenge string) bool {
	return p.SpaceID == s.SpaceID() && p.DeviceID == s.DeviceID() && p.DeviceID == c.DeviceID && p.Issuer == c.Issuer && p.AccountID == c.AccountID && p.TenantID == c.TenantID && p.SessionID == c.SessionID && p.Endpoint == c.Endpoint && p.Mode == mode && p.ChallengeID == challenge && p.RedirectURI == c.RedirectURI
}
func (a Auth) readDeletion(key string) (deletionChallenge, error) {
	raw, e := a.vault().Get(key)
	if e != nil {
		return deletionChallenge{}, ErrApproval
	}
	var p deletionChallenge
	if json.Unmarshal([]byte(raw), &p) != nil || p.Used || !time.Now().Before(p.ExpiresAt) {
		return deletionChallenge{}, ErrApproval
	}
	return p, nil
}

// CompleteDeletion stores the proof privately and returns only the challenge ID.
// Receiving a browser callback never authorizes this helper to delete data.
func (a Auth) CompleteDeletion(ctx context.Context, s Store, c Credential, callbackURI string) (string, error) {
	u, e := url.Parse(callbackURI)
	if e != nil {
		return "", ErrApproval
	}
	q, e := url.ParseQuery(u.RawQuery)
	if e != nil || len(q["reauth_challenge"]) != 1 || u.Fragment != "" {
		return "", ErrApproval
	}
	challenge := q.Get("reauth_challenge")
	if !validUUID(challenge) {
		return "", ErrApproval
	}
	key := deletionKey(s.SpaceID(), challenge)
	unlock, e := a.lockCredential(ctx, key)
	if e != nil {
		return "", e
	}
	defer unlock()
	pending, e := a.readDeletion(key)
	if e != nil {
		return "", e
	}
	u.RawQuery = ""
	if !pending.matches(s, c, pending.Mode, challenge) || u.String() != pending.RedirectURI {
		return "", ErrApproval
	}
	if pending.Proof == "" {
		proof, e := a.ReauthenticationProof(ctx, c, challenge, callbackURI)
		if e != nil {
			return "", e
		}
		pending.Proof = proof
		raw, e := json.Marshal(pending)
		if e != nil {
			return "", e
		}
		if e = a.vault().Set(key, string(raw)); e != nil {
			return "", e
		}
	}
	return challenge, nil
}

// ConsumeDeletionProof is for the internal AccountService adapter only. It
// marks the proof used before dispatch; a failed RPC needs fresh reauthentication.
func (a Auth) ConsumeDeletionProof(ctx context.Context, s Store, c Credential, mode lanev1.DataDeletionMode, challengeID string) (string, error) {
	key := deletionKey(s.SpaceID(), challengeID)
	unlock, e := a.lockCredential(ctx, key)
	if e != nil {
		return "", e
	}
	defer unlock()
	pending, e := a.readDeletion(key)
	if e != nil {
		return "", e
	}
	if !pending.matches(s, c, mode, challengeID) || pending.Proof == "" {
		return "", ErrApproval
	}
	proof := pending.Proof
	pending.Proof = ""
	pending.Used = true
	raw, e := json.Marshal(pending)
	if e != nil {
		return "", e
	}
	if e = a.vault().Set(key, string(raw)); e != nil {
		return "", e
	}
	return proof, nil
}

// CancelDeletion invalidates exactly one bound challenge without affecting a
// newer approval. Repeated cancellation cannot restore or expose its proof.
func (a Auth) CancelDeletion(ctx context.Context, s Store, c Credential, challengeID string) error {
	key := deletionKey(s.SpaceID(), challengeID)
	unlock, e := a.lockCredential(ctx, key)
	if e != nil {
		return e
	}
	defer unlock()
	raw, e := a.vault().Get(key)
	if e != nil {
		return ErrApproval
	}
	var pending deletionChallenge
	if json.Unmarshal([]byte(raw), &pending) != nil || !pending.matches(s, c, pending.Mode, challengeID) {
		return ErrApproval
	}
	pending.Proof = ""
	pending.Used = true
	encoded, e := json.Marshal(pending)
	if e != nil {
		return e
	}
	return a.vault().Set(key, string(encoded))
}
