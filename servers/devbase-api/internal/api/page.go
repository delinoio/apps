// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import (
	"encoding/base64"
	"strconv"
	"strings"
	"time"
)

// cursor is the keyset position after the last returned row.
type cursor struct {
	At time.Time
	ID string
}

func pageSize(n int32) (int, error) {
	switch {
	case n == 0:
		return defaultPage, nil
	case n < 0 || n > maxPageSize:
		return 0, invalid("page_size must be 1 to 100")
	}
	return int(n), nil
}

func encodeCursor(at time.Time, id string) string {
	return base64.RawURLEncoding.EncodeToString([]byte(strconv.FormatInt(at.UnixMicro(), 10) + "." + id))
}

// decodeCursor returns nil for an empty token and invalid_argument for a malformed one.
func decodeCursor(token string) (*cursor, error) {
	if token == "" {
		return nil, nil
	}
	raw, e := base64.RawURLEncoding.DecodeString(token)
	if e != nil {
		return nil, invalid("page_token is invalid")
	}
	micro, id, ok := strings.Cut(string(raw), ".")
	n, e := strconv.ParseInt(micro, 10, 64)
	if !ok || e != nil || !canonicalUUID(id) {
		return nil, invalid("page_token is invalid")
	}
	return &cursor{At: time.UnixMicro(n).UTC(), ID: id}, nil
}

// args returns the SQL arguments for keyset conditions: an optional timestamp and ID.
func (c *cursor) args() (*time.Time, string) {
	if c == nil {
		return nil, "00000000-0000-0000-0000-000000000000"
	}
	return &c.At, c.ID
}
