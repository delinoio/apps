// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import (
	"net/mail"
	"regexp"
	"strings"
	"unicode/utf8"

	"github.com/google/uuid"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/structpb"
)

const (
	maxName         = 256
	maxEmail        = 254
	maxMetadata     = 16 << 10
	maxRequestBytes = 64 << 10
	maxPageSize     = 100
	defaultPage     = 50
)

var idempotencyPattern = regexp.MustCompile(`^[A-Za-z0-9._:-]{8,128}$`)

func canonicalUUID(s string) bool {
	u, e := uuid.Parse(s)
	return e == nil && u.Version() == 7 && u.String() == s
}

func requireID(s, field string) error {
	if !canonicalUUID(s) {
		return invalid(field + " must be a canonical UUID v7")
	}
	return nil
}

func requestSize(m proto.Message) error {
	if proto.Size(m) > maxRequestBytes {
		return invalid("request exceeds 64 KiB")
	}
	return nil
}

func idempotencyKey(s string) error {
	if !idempotencyPattern.MatchString(s) {
		return invalid("idempotency_key must contain 8 to 128 characters from A-Z a-z 0-9 . _ : -")
	}
	return nil
}

// identifier validates a required developer-visible identifier.
func identifier(s, field string) error {
	if s == "" || len(s) > maxName || !utf8.ValidString(s) || strings.ContainsRune(s, 0) {
		return invalid(field + " must be 1 to 256 bytes of valid UTF-8")
	}
	if strings.TrimSpace(s) != s {
		return invalid(field + " must not have leading or trailing whitespace")
	}
	return nil
}

func projectName(s string) error { return identifier(s, "name") }
func externalID(s string) error  { return identifier(s, "external_id") }

func userName(s string) error {
	if len(s) > maxName || !utf8.ValidString(s) || strings.ContainsRune(s, 0) {
		return invalid("name must be at most 256 bytes of valid UTF-8")
	}
	return nil
}

func userEmail(s string) error {
	if s == "" {
		return nil
	}
	if len(s) > maxEmail || !utf8.ValidString(s) || strings.ContainsAny(s, " \t\r\n<>,;\x00") {
		return invalid("email must be a valid address of at most 254 bytes")
	}
	a, e := mail.ParseAddress(s)
	if e != nil || a.Address != s || a.Name != "" {
		return invalid("email must be a valid address of at most 254 bytes")
	}
	return nil
}

// metadataJSON validates the object and returns its canonical JSON text.
func metadataJSON(m *structpb.Struct) ([]byte, error) {
	if m == nil {
		return []byte("{}"), nil
	}
	b, e := protojson.Marshal(m)
	if e != nil || len(b) > maxMetadata || strings.ContainsRune(string(b), 0) {
		return nil, invalid("metadata must be a JSON object of at most 16 KiB")
	}
	return b, nil
}
