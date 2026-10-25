// SPDX-License-Identifier: Apache-2.0
package store

import (
	"bytes"
	"connectrpc.com/connect"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	lane "github.com/delinoio/apps/protos/lane/v1"
	"golang.org/x/net/idna"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"net"
	"net/url"
	"sort"
	"strconv"
	"strings"
	"unicode/utf16"
)

func domainError(reason lane.ErrorReason, message, id string, heads ...string) error {
	code := connect.CodeFailedPrecondition
	switch reason {
	case 1:
		code = connect.CodeInvalidArgument
	case 2:
		code = connect.CodeNotFound
	case 3, 5:
		code = connect.CodeAborted
	case 6:
		code = connect.CodeAlreadyExists
	case 9, 14:
		code = connect.CodeUnavailable
	case 10, 15:
		code = connect.CodeResourceExhausted
	}
	e := connect.NewError(code, errors.New(message))
	d, _ := connect.NewErrorDetail(&lane.ErrorDetail{Reason: reason, CorrelationId: NewID(), EntityId: id, CurrentHeadIds: heads, Retryable: reason == 9})
	e.AddDetail(d)
	return e
}

// NormalizeURL converts transport URLs to the repository identity without Git access.
func NormalizeURL(raw string) (string, error) {
	bad := func() (string, error) { return "", domainError(1, "Invalid repository URL", "") }
	if strings.ContainsAny(raw, "\\?#") {
		return bad()
	}
	if !strings.Contains(raw, "://") {
		at := strings.Index(raw, "@")
		colon := strings.Index(raw, ":")
		if at < 1 || colon < at {
			return bad()
		}
		raw = "ssh://" + raw[:colon] + "/" + raw[colon+1:]
	}
	u, e := url.Parse(raw)
	if e != nil || u.Host == "" || (u.Scheme != "https" && u.Scheme != "ssh") || u.RawQuery != "" || u.Fragment != "" {
		return bad()
	}
	host, e := idna.Lookup.ToASCII(strings.ToLower(u.Hostname()))
	if e != nil || host == "" {
		return bad()
	}
	port := u.Port()
	if port != "" {
		n, e := strconv.Atoi(port)
		if e != nil || n < 1 || n > 65535 {
			return bad()
		}
	}
	if (u.Scheme == "https" && port == "443") || (u.Scheme == "ssh" && port == "22") {
		port = ""
	}
	if strings.Contains(host, ":") {
		host = "[" + host + "]"
	}
	if port != "" {
		host = net.JoinHostPort(strings.Trim(host, "[]"), port)
	}
	rawPath := raw[strings.Index(raw, "://")+3:]
	if slash := strings.Index(rawPath, "/"); slash >= 0 {
		rawPath = rawPath[slash:]
	} else {
		rawPath = ""
	}
	path := strings.Trim(rawPath, "/")
	var b strings.Builder
	for i := 0; i < len(path); i++ {
		if path[i] != '%' {
			b.WriteByte(path[i])
			continue
		}
		if i+2 >= len(path) {
			return bad()
		}
		v, e := strconv.ParseUint(path[i+1:i+3], 16, 8)
		if e != nil || v == '/' || v == '\\' {
			return bad()
		}
		x := byte(v)
		if x >= 'a' && x <= 'z' || x >= 'A' && x <= 'Z' || x >= '0' && x <= '9' || strings.ContainsRune("-._~", rune(x)) {
			b.WriteByte(x)
		} else {
			b.WriteString("%" + strings.ToUpper(path[i+1:i+3]))
		}
		i += 2
	}
	path = b.String() // Preserve literal Unicode path text rather than URL encoder escapes.
	for _, seg := range strings.Split(path, "/") {
		if seg == "." || seg == ".." {
			return bad()
		}
	}
	path = strings.TrimSuffix(path, ".git")
	if path == "" {
		return bad()
	}
	return host + "/" + path, nil
}
func marshal(m proto.Message) string {
	b, e := protojson.Marshal(m)
	if e != nil {
		panic(e)
	}
	return string(b)
}
func fingerprint(m proto.Message) (string, error) {
	b, e := (protojson.MarshalOptions{EmitDefaultValues: true}).Marshal(m)
	if e != nil {
		return "", e
	}
	var v any
	if e = json.Unmarshal(b, &v); e != nil {
		return "", e
	}
	var out bytes.Buffer
	canonical(&out, v)
	sum := sha256.Sum256(out.Bytes())
	return hex.EncodeToString(sum[:]), nil
}
func canonical(b *bytes.Buffer, v any) {
	switch x := v.(type) {
	case map[string]any:
		keys := make([]string, 0, len(x))
		for k := range x {
			keys = append(keys, k)
		}
		sort.Slice(keys, func(i, j int) bool {
			a, c := utf16.Encode([]rune(keys[i])), utf16.Encode([]rune(keys[j]))
			for n := 0; n < len(a) && n < len(c); n++ {
				if a[n] != c[n] {
					return a[n] < c[n]
				}
			}
			return len(a) < len(c)
		})
		b.WriteByte('{')
		for i, k := range keys {
			if i > 0 {
				b.WriteByte(',')
			}
			canonical(b, k)
			b.WriteByte(':')
			canonical(b, x[k])
		}
		b.WriteByte('}')
	case []any:
		b.WriteByte('[')
		for i, y := range x {
			if i > 0 {
				b.WriteByte(',')
			}
			canonical(b, y)
		}
		b.WriteByte(']')
	default:
		var t bytes.Buffer
		e := json.NewEncoder(&t)
		e.SetEscapeHTML(false)
		_ = e.Encode(v)
		b.Write(bytes.TrimSuffix(t.Bytes(), []byte("\n")))
	}
}
