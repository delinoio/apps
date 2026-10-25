// SPDX-License-Identifier: Apache-2.0

package mcp

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"strings"
	"testing"
	"time"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
)

const initialize = `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}` + "\n"

func run(t *testing.T, input string, invoke Invoke) []map[string]any {
	t.Helper()
	var out bytes.Buffer
	if err := Serve(context.Background(), strings.NewReader(input), &out, invoke); err != nil {
		t.Fatal(err)
	}
	var results []map[string]any
	scanner := bufio.NewScanner(&out)
	scanner.Buffer(make([]byte, 4096), maxFrameBytes)
	for scanner.Scan() {
		var r map[string]any
		if err := json.Unmarshal(scanner.Bytes(), &r); err != nil {
			t.Fatal(err)
		}
		results = append(results, r)
	}
	if err := scanner.Err(); err != nil {
		t.Fatal(err)
	}
	return results
}

func TestFramingAndResults(t *testing.T) {
	input := initialize + `{"jsonrpc":"2.0","method":"notifications/initialized"}` + "\n" + `{"jsonrpc":"2.0","id":"ping","method":"ping"}` + "\r\n" + `{"jsonrpc":"2.0","id":3,"method":"tools/list"}` + "\n" + `{"jsonrpc":"2.0","id":"call","method":"tools/call","params":{"name":"lane_get_task","arguments":{"id":"task"}}}`
	calls := 0
	results := run(t, input, func(ctx context.Context, method string, args json.RawMessage) (any, error) {
		calls++
		if method != "GetTask" || string(args) != `{"id":"task"}` {
			t.Errorf("unexpected dispatch %s %s", method, args)
		}
		return &lanev1.GetTaskResponse{Result: &lanev1.EntityResult{Result: lanev1.ResultKind_RESULT_KIND_APPLIED}}, nil
	})
	if len(results) != 4 || calls != 1 {
		t.Fatalf("responses=%d calls=%d", len(results), calls)
	}
	for _, r := range results {
		if r["jsonrpc"] != "2.0" {
			t.Fatal(r)
		}
		if r["id"] == "call" {
			result := r["result"].(map[string]any)
			structured := result["structuredContent"].(map[string]any)
			if structured["result"].(map[string]any)["result"] != "RESULT_KIND_APPLIED" {
				t.Fatal(structured)
			}
			text := result["content"].([]any)[0].(map[string]any)["text"].(string)
			var v any
			if json.Unmarshal([]byte(text), &v) != nil {
				t.Fatal(text)
			}
		}
	}
}

func TestProtocolErrorsAndNotifications(t *testing.T) {
	cases := []struct {
		body string
		code float64
	}{
		{`{`, -32700}, {`[]`, -32600}, {`{"jsonrpc":"2.0","id":null,"method":"ping"}`, -32600},
		{`{"jsonrpc":"2.0","id":2,"method":"missing"}`, -32601},
		{`{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lane_fake","arguments":{}}}`, -32602},
		{`{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lane_get_task","arguments":{"origin":{}}}}`, -32602},
	}
	for _, tc := range cases {
		t.Run(tc.body, func(t *testing.T) {
			r := run(t, initialize+tc.body+"\n", func(context.Context, string, json.RawMessage) (any, error) {
				t.Fatal("invalid call dispatched")
				return nil, nil
			})
			if len(r) != 2 || r[1]["error"].(map[string]any)["code"] != tc.code {
				t.Fatal(r)
			}
		})
	}
	r := run(t, initialize+`{"jsonrpc":"2.0","method":"tools/call","params":{"name":"lane_remove_device_data"}}`+"\n"+`{"jsonrpc":"2.0","method":"unknown"}`+"\n", func(context.Context, string, json.RawMessage) (any, error) {
		t.Fatal("notification invoked mutation")
		return nil, nil
	})
	if len(r) != 1 {
		t.Fatal(r)
	}
}

func TestToolFailureAndMarshalFailure(t *testing.T) {
	for _, tc := range []struct {
		name     string
		fn       Invoke
		protocol bool
	}{
		{"domain", func(context.Context, string, json.RawMessage) (any, error) { return nil, errors.New("ONLINE_REQUIRED") }, false},
		{"encode", func(context.Context, string, json.RawMessage) (any, error) { return make(chan int), nil }, true},
		{"panic", func(context.Context, string, json.RawMessage) (any, error) { panic("private details") }, true},
		{"scalar", func(context.Context, string, json.RawMessage) (any, error) { return 1, nil }, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r := run(t, initialize+`{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lane_get_local_status"}}`+"\n", tc.fn)
			if tc.protocol {
				if r[1]["error"].(map[string]any)["code"] != float64(-32603) {
					t.Fatal(r)
				}
			} else {
				result := r[1]["result"].(map[string]any)
				if result["isError"] != true || result["structuredContent"] == nil || result["content"] == nil {
					t.Fatal(result)
				}
			}
		})
	}
}

func TestCancellation(t *testing.T) {
	reader, writer := io.Pipe()
	outReader, outWriter := io.Pipe()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	started := make(chan struct{})
	cancelled := make(chan struct{})
	finished := make(chan error, 1)
	go func() {
		finished <- Serve(ctx, reader, outWriter, func(ctx context.Context, _ string, _ json.RawMessage) (any, error) {
			close(started)
			<-ctx.Done()
			close(cancelled)
			return nil, ctx.Err()
		})
		outWriter.Close()
	}()
	lines := make(chan string, 10)
	go func() {
		s := bufio.NewScanner(outReader)
		for s.Scan() {
			lines <- s.Text()
		}
		close(lines)
	}()
	io.WriteString(writer, initialize)
	io.WriteString(writer, `{"jsonrpc":"2.0","id":"work","method":"tools/call","params":{"name":"lane_get_local_status"}}`+"\n")
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("call not started")
	}
	io.WriteString(writer, `{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"\u0077ork"}}`+"\n")
	select {
	case <-cancelled:
	case <-time.After(time.Second):
		t.Fatal("call not cancelled")
	}
	writer.Close()
	select {
	case err := <-finished:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(time.Second):
		t.Fatal("EOF did not stop server")
	}
	count := 0
	for range lines {
		count++
	}
	if count != 1 {
		t.Fatalf("cancelled request produced reply: %d", count)
	}
}

func TestContextCancellationUnblocksPipe(t *testing.T) {
	r, w := io.Pipe()
	defer w.Close()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- Serve(ctx, r, io.Discard, func(context.Context, string, json.RawMessage) (any, error) { return nil, nil })
	}()
	cancel()
	select {
	case err := <-done:
		if !errors.Is(err, context.Canceled) {
			t.Fatal(err)
		}
	case <-time.After(time.Second):
		t.Fatal("blocked read")
	}
}

type failingWriter struct{}

func (failingWriter) Write([]byte) (int, error) { return 0, io.ErrClosedPipe }
func TestIOErrors(t *testing.T) {
	invoke := func(context.Context, string, json.RawMessage) (any, error) { return map[string]any{}, nil }
	if err := Serve(context.Background(), strings.NewReader(initialize), failingWriter{}, invoke); !errors.Is(err, io.ErrClosedPipe) {
		t.Fatal(err)
	}
	if err := Serve(context.Background(), strings.NewReader(strings.Repeat("x", maxFrameBytes+1)), io.Discard, invoke); err == nil {
		t.Fatal("oversized frame accepted")
	}
}

type shortWriter struct{}

func (shortWriter) Write(p []byte) (int, error) { return len(p) - 1, nil }
func TestShortWrite(t *testing.T) {
	err := Serve(context.Background(), strings.NewReader(initialize), shortWriter{}, func(context.Context, string, json.RawMessage) (any, error) { return nil, nil })
	if !errors.Is(err, io.ErrShortWrite) {
		t.Fatal(err)
	}
}

func TestTypedDecimalResult(t *testing.T) {
	result, err := call(context.Background(), func(context.Context, string, json.RawMessage) (any, error) {
		return &lanev1.SaveReceiverResponse{Receiver: &lanev1.WebhookReceiver{Revision: 18446744073709551615}}, nil
	}, "SaveReceiver", json.RawMessage(`{}`))
	if err != nil {
		t.Fatal(err)
	}
	content := result.(map[string]any)["structuredContent"].(map[string]any)
	if content["receiver"].(map[string]any)["revision"] != "18446744073709551615" {
		t.Fatal(content)
	}
}

type structuredFailure struct{}

func (structuredFailure) Error() string { return "conflict" }
func (structuredFailure) ErrorDetails() any {
	return map[string]any{"reason": "ERROR_REASON_STALE_VERSION", "currentHeadIds": []string{"a", "b"}}
}
func TestStructuredDomainFailure(t *testing.T) {
	result, rpcErr := call(context.Background(), func(context.Context, string, json.RawMessage) (any, error) { return nil, structuredFailure{} }, "SaveTask", json.RawMessage(`{}`))
	if rpcErr != nil {
		t.Fatal(rpcErr)
	}
	encoded, err := json.Marshal(result)
	if err != nil || !strings.Contains(string(encoded), "ERROR_REASON_STALE_VERSION") || !strings.Contains(string(encoded), "currentHeadIds") {
		t.Fatalf("%s %v", encoded, err)
	}
}
