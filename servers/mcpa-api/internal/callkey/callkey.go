// SPDX-License-Identifier: FSL-1.1-ALv2

// Package callkey generates and parses mcpa call keys.
//
// A call key has the form
//
//	mcpa_<32 lowercase hex of the key UUID v7 without hyphens>_<43 base64url chars>
//
// The middle part identifies the key row; the last part carries 32 random
// bytes. mcpa stores only the SHA-256 digest of the full secret.
package callkey

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"strings"

	"github.com/google/uuid"
)

const (
	// Prefix starts every call key.
	Prefix    = "mcpa_"
	idHexLen  = 32
	secretLen = 43
	// Length is the total length of a call key.
	Length = len(Prefix) + idHexLen + 1 + secretLen
)

// ErrMalformed means a presented value is not a well-formed call key.
var ErrMalformed = errors.New("callkey: malformed call key")

// Key is a newly generated call key.
type Key struct {
	ID     uuid.UUID
	Secret string
	Hash   []byte
}

// New generates a key with a fresh UUID v7 identifier.
func New() (Key, error) {
	id, err := uuid.NewV7()
	if err != nil {
		return Key{}, err
	}
	return NewWithID(id)
}

// NewWithID generates a key for an existing identifier.
func NewWithID(id uuid.UUID) (Key, error) {
	random := make([]byte, 32)
	if _, err := rand.Read(random); err != nil {
		return Key{}, err
	}
	secret := Prefix + hex.EncodeToString(id[:]) + "_" + base64.RawURLEncoding.EncodeToString(random)
	return Key{ID: id, Secret: secret, Hash: Hash(secret)}, nil
}

// Hash returns the stored SHA-256 digest of a full secret.
func Hash(secret string) []byte {
	sum := sha256.Sum256([]byte(secret))
	return sum[:]
}

// Parse validates the format of a presented key and returns its key ID. It
// does not verify the secret; compare Hash(secret) with Verify.
func Parse(secret string) (uuid.UUID, error) {
	if len(secret) != Length || !strings.HasPrefix(secret, Prefix) {
		return uuid.Nil, ErrMalformed
	}
	rest := secret[len(Prefix):]
	idHex, random := rest[:idHexLen], rest[idHexLen:]
	if random[0] != '_' {
		return uuid.Nil, ErrMalformed
	}
	random = random[1:]
	for i := 0; i < len(idHex); i++ {
		c := idHex[i]
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
			return uuid.Nil, ErrMalformed
		}
	}
	raw, err := base64.RawURLEncoding.Strict().DecodeString(random)
	if err != nil || len(raw) != 32 {
		return uuid.Nil, ErrMalformed
	}
	var id uuid.UUID
	if _, err := hex.Decode(id[:], []byte(idHex)); err != nil {
		return uuid.Nil, ErrMalformed
	}
	if id.Version() != 7 || id.Variant() != uuid.RFC4122 {
		return uuid.Nil, ErrMalformed
	}
	return id, nil
}

// Verify compares a presented secret with a stored digest in constant time.
func Verify(secret string, storedHash []byte) bool {
	return subtle.ConstantTimeCompare(Hash(secret), storedHash) == 1
}

// DisplayPrefix returns the identifying prefix shown after issuance.
func DisplayPrefix(id uuid.UUID) string {
	return Prefix + hex.EncodeToString(id[:])[:8] + "…"
}
