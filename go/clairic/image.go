// SPDX-License-Identifier: Apache-2.0

package clairic

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"strings"
)

// Image accepts either DataURL or the ContentType and Base64 pair.
// Supported media types are image/png, image/jpeg and image/webp.
type Image struct {
	DataURL     string
	ContentType string
	Base64      string
}

// MarshalJSON emits the string or object form required by Clairic.
func (im Image) MarshalJSON() ([]byte, error) {
	if _, err := im.decodedSize(); err != nil {
		return nil, err
	}
	if im.DataURL != "" {
		return json.Marshal(im.DataURL)
	}
	return json.Marshal(struct {
		ContentType string `json:"content_type"`
		Base64      string `json:"base64"`
	}{im.ContentType, im.Base64})
}

func (im Image) decodedSize() (int, error) {
	media, data := im.ContentType, im.Base64
	if im.DataURL != "" {
		if media != "" || data != "" {
			return 0, fmt.Errorf("clairic: image must use only one representation")
		}
		header, encoded, ok := strings.Cut(im.DataURL, ",")
		if !ok || !strings.HasPrefix(strings.ToLower(header), "data:") || !strings.HasSuffix(header, ";base64") {
			return 0, fmt.Errorf("clairic: image must be a base64 data URL")
		}
		media, data = header[5:len(header)-7], encoded
	}
	switch media {
	case "image/png", "image/jpeg", "image/webp":
	default:
		return 0, fmt.Errorf("clairic: unsupported image content type")
	}
	// Bound allocations before decoding untrusted input.
	if len(data) > base64.StdEncoding.EncodedLen(4<<20) {
		return 0, fmt.Errorf("clairic: image exceeds 4 MiB")
	}
	decoded, err := base64.StdEncoding.DecodeString(data)
	if err != nil {
		return 0, fmt.Errorf("clairic: invalid image base64: %w", err)
	}
	if len(decoded) == 0 || len(decoded) > 4<<20 {
		return 0, fmt.Errorf("clairic: image must contain 1 byte to 4 MiB")
	}
	return len(decoded), nil
}
