// SPDX-License-Identifier: FSL-1.1-ALv2

package main

import (
	"bytes"
	"encoding/json"
	"strings"
	"testing"
)

func TestLoggerFormats(t *testing.T) {
	var out bytes.Buffer
	logger, e := newLogger("json", false, &out)
	if e != nil {
		t.Fatal(e)
	}
	logger.Info("request completed", "trace_id", "t1")
	var record map[string]any
	if e = json.Unmarshal(out.Bytes(), &record); e != nil || record["trace_id"] != "t1" || strings.Contains(out.String(), "\x1b") {
		t.Fatalf("JSON mode must emit one plain JSON object per record: %q", out.String())
	}
	out.Reset()
	colored, _ := newLogger("", false, &out)
	colored.Info("started")
	if !strings.Contains(out.String(), "\x1b[") {
		t.Fatal("text mode must be colored by default")
	}
	out.Reset()
	plain, _ := newLogger("text", true, &out)
	plain.Info("started")
	if strings.Contains(out.String(), "\x1b") || !strings.Contains(out.String(), "msg=started") {
		t.Fatalf("NO_COLOR must produce plain key/value text: %q", out.String())
	}
	if _, e = newLogger("xml", false, &out); e == nil {
		t.Fatal("unknown format accepted")
	}
}
