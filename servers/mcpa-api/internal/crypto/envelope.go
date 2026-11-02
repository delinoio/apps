// SPDX-License-Identifier: FSL-1.1-ALv2

// Package crypto implements envelope encryption for stored secrets.
//
// Every secret gets its own random 32-byte data key. The secret is sealed with
// AES-256-GCM under the data key, and the data key is wrapped with AES-256-GCM
// under the active master key. Both operations bind the caller-supplied
// additional data (for example the credential and resource IDs), so a sealed
// value cannot be moved to another row. Master key rotation re-wraps only the
// data key; the secret ciphertext is unchanged.
package crypto

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"errors"
	"fmt"
	"sort"
)

const (
	keySize = 32
	// wrapDomain separates data-key wrapping from secret sealing so a wrapped
	// key can never be opened as a secret with the same AAD.
	wrapDomain = "mcpa/data-key/v1\x00"
	sealDomain = "mcpa/secret/v1\x00"
)

// ErrUnknownKey means a sealed value references a master key that is not
// configured. Operators must keep old keys until rotation completes.
var ErrUnknownKey = errors.New("crypto: master key is not configured")

// ErrDecrypt means authentication failed. The data was modified, bound to
// other additional data, or wrapped under a different key.
var ErrDecrypt = errors.New("crypto: decryption failed")

// Sealed is the stored form of one secret.
type Sealed struct {
	KeyID      string
	WrappedKey []byte
	KeyNonce   []byte
	Nonce      []byte
	Ciphertext []byte
}

// Keyring holds the configured master keys.
type Keyring struct {
	keys   map[string]cipher.AEAD
	active string
}

// NewKeyring validates the master keys and the active key ID.
func NewKeyring(keys map[string][]byte, active string) (*Keyring, error) {
	if len(keys) == 0 {
		return nil, errors.New("crypto: no master keys")
	}
	k := &Keyring{keys: make(map[string]cipher.AEAD, len(keys)), active: active}
	for id, key := range keys {
		aead, err := newAEAD(key)
		if err != nil {
			return nil, fmt.Errorf("crypto: master key %q: %w", id, err)
		}
		k.keys[id] = aead
	}
	if _, ok := k.keys[active]; !ok {
		return nil, fmt.Errorf("crypto: active key %q is not configured", active)
	}
	return k, nil
}

// ActiveKeyID returns the key ID used for new encryptions.
func (k *Keyring) ActiveKeyID() string { return k.active }

// KeyIDs returns the configured key IDs in sorted order.
func (k *Keyring) KeyIDs() []string {
	ids := make([]string, 0, len(k.keys))
	for id := range k.keys {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	return ids
}

func newAEAD(key []byte) (cipher.AEAD, error) {
	if len(key) != keySize {
		return nil, errors.New("key must be 32 bytes")
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	return cipher.NewGCM(block)
}

func randomBytes(n int) ([]byte, error) {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		return nil, err
	}
	return b, nil
}

func withDomain(domain string, aad []byte) []byte {
	out := make([]byte, 0, len(domain)+len(aad))
	out = append(out, domain...)
	return append(out, aad...)
}

// Seal encrypts plaintext under a fresh data key wrapped by the active key.
func (k *Keyring) Seal(plaintext, aad []byte) (Sealed, error) {
	dataKey, err := randomBytes(keySize)
	if err != nil {
		return Sealed{}, err
	}
	defer clear(dataKey)
	dataAEAD, err := newAEAD(dataKey)
	if err != nil {
		return Sealed{}, err
	}
	nonce, err := randomBytes(dataAEAD.NonceSize())
	if err != nil {
		return Sealed{}, err
	}
	s := Sealed{
		Nonce:      nonce,
		Ciphertext: dataAEAD.Seal(nil, nonce, plaintext, withDomain(sealDomain, aad)),
	}
	if err := k.wrap(&s, dataKey, aad); err != nil {
		return Sealed{}, err
	}
	return s, nil
}

func (k *Keyring) wrap(s *Sealed, dataKey, aad []byte) error {
	master := k.keys[k.active]
	keyNonce, err := randomBytes(master.NonceSize())
	if err != nil {
		return err
	}
	s.KeyID = k.active
	s.KeyNonce = keyNonce
	s.WrappedKey = master.Seal(nil, keyNonce, dataKey, withDomain(wrapDomain+s.KeyID+"\x00", aad))
	return nil
}

func (k *Keyring) unwrap(s Sealed, aad []byte) ([]byte, error) {
	master, ok := k.keys[s.KeyID]
	if !ok {
		return nil, ErrUnknownKey
	}
	if len(s.KeyNonce) != master.NonceSize() {
		return nil, ErrDecrypt
	}
	dataKey, err := master.Open(nil, s.KeyNonce, s.WrappedKey, withDomain(wrapDomain+s.KeyID+"\x00", aad))
	if err != nil || len(dataKey) != keySize {
		return nil, ErrDecrypt
	}
	return dataKey, nil
}

// Open decrypts a sealed value bound to aad.
func (k *Keyring) Open(s Sealed, aad []byte) ([]byte, error) {
	dataKey, err := k.unwrap(s, aad)
	if err != nil {
		return nil, err
	}
	defer clear(dataKey)
	dataAEAD, err := newAEAD(dataKey)
	if err != nil {
		return nil, err
	}
	if len(s.Nonce) != dataAEAD.NonceSize() {
		return nil, ErrDecrypt
	}
	plaintext, err := dataAEAD.Open(nil, s.Nonce, s.Ciphertext, withDomain(sealDomain, aad))
	if err != nil {
		return nil, ErrDecrypt
	}
	return plaintext, nil
}

// Rewrap re-wraps the data key of s under the active master key. The secret
// ciphertext and nonce are unchanged. A value already under the active key is
// returned as is with changed=false.
func (k *Keyring) Rewrap(s Sealed, aad []byte) (out Sealed, changed bool, err error) {
	if s.KeyID == k.active {
		return s, false, nil
	}
	dataKey, err := k.unwrap(s, aad)
	if err != nil {
		return Sealed{}, false, err
	}
	defer clear(dataKey)
	out = Sealed{Nonce: s.Nonce, Ciphertext: s.Ciphertext}
	if err := k.wrap(&out, dataKey, aad); err != nil {
		return Sealed{}, false, err
	}
	return out, true, nil
}

// CredentialAAD binds a provider credential to its row and resource.
func CredentialAAD(credentialID, resourceID string) []byte {
	return []byte("credential\x00" + credentialID + "\x00" + resourceID)
}

// OAuthFlowAAD binds provider OAuth flow secrets to their row and resource.
func OAuthFlowAAD(flowID, resourceID string) []byte {
	return []byte("provider-oauth-flow\x00" + flowID + "\x00" + resourceID)
}

// SessionAAD binds console session tokens to their session row.
func SessionAAD(sessionID string) []byte {
	return []byte("console-session\x00" + sessionID)
}

// MCPGrantAAD binds MCP OAuth broker tokens to their grant row.
func MCPGrantAAD(grantID string) []byte {
	return []byte("mcp-grant\x00" + grantID)
}
