// SPDX-License-Identifier: Apache-2.0

// Package mcp provides the Lane stdio protocol adapter without owning a store.
package mcp

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"

	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
)

// Invoke dispatches a short method name and validated camelCase arguments.
// The caller must enforce the selected target, authorization, and real MCP
// origin recursively. It must honor context cancellation. Results must be a
// proto.Message or a JSON object. Errors must be safe to expose to the client.
type Invoke func(context.Context, string, json.RawMessage) (any, error)

const maxFrameBytes = 4 << 20
const maxConcurrent = 32
const protocolVersion = "2025-06-18"

type rpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}
type response struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id"`
	Result  any             `json:"result,omitempty"`
	Error   *rpcError       `json:"error,omitempty"`
}
type request struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id"`
	Method  string          `json:"method"`
	Params  json.RawMessage `json:"params"`
}
type completion struct {
	key    string
	result any
	err    *rpcError
}
type pending struct {
	id        json.RawMessage
	cancel    context.CancelFunc
	cancelled bool
}
type frame struct {
	data []byte
	err  error
}

// Serve runs newline-delimited JSON-RPC. EOF drains accepted calls; cancellation
// stops them. If in implements io.Closer, Serve closes it on exit to unblock a
// pending read. For other readers, the caller must unblock Read on cancellation.
// Serve does not close out. No diagnostic output is written to either stream.
func Serve(ctx context.Context, in io.Reader, out io.Writer, invoke Invoke) error {
	return ServeVersion(ctx, in, out, invoke, "0.1.0")
}

// ServeVersion reports the executable release version in the MCP handshake.
func ServeVersion(ctx context.Context, in io.Reader, out io.Writer, invoke Invoke, version string) error {
	if in == nil || out == nil || invoke == nil {
		return errors.New("MCP requires input, output, and an invocation dispatcher")
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	if c, ok := in.(io.Closer); ok {
		defer c.Close()
	}
	frames := make(chan frame)
	go func() {
		scanner := bufio.NewScanner(in)
		scanner.Buffer(make([]byte, 4096), maxFrameBytes)
		for scanner.Scan() {
			f := frame{data: bytes.Clone(scanner.Bytes())}
			select {
			case frames <- f:
			case <-ctx.Done():
				return
			}
		}
		err := scanner.Err()
		if err == nil {
			err = io.EOF
		}
		select {
		case frames <- frame{err: err}:
		case <-ctx.Done():
		}
	}()
	tools := Tools()
	catalog := map[string]Tool{}
	for _, t := range tools {
		catalog[t.Name] = t
	}
	active := map[string]*pending{}
	defer func() {
		for _, p := range active {
			p.cancel()
		}
	}()
	done := make(chan completion, maxConcurrent)
	write := func(id json.RawMessage, result any, e *rpcError) error {
		if len(id) == 0 {
			id = json.RawMessage("null")
		}
		data, err := json.Marshal(response{"2.0", id, result, e})
		if err != nil {
			return err
		}
		data = append(data, '\n')
		n, err := out.Write(data)
		if err == nil && n != len(data) {
			err = io.ErrShortWrite
		}
		return err
	}
	initialized := false
	var inputErr error
	for {
		if frames == nil && len(active) == 0 {
			if errors.Is(inputErr, io.EOF) {
				return nil
			}
			return inputErr
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case c := <-done:
			p := active[c.key]
			if p == nil {
				continue
			}
			delete(active, c.key)
			p.cancel()
			if !p.cancelled {
				if err := write(p.id, c.result, c.err); err != nil {
					return err
				}
			}
		case f := <-frames:
			if f.err != nil {
				inputErr = f.err
				frames = nil
				if !errors.Is(f.err, io.EOF) {
					return f.err
				}
				continue
			}
			if len(bytes.TrimSpace(f.data)) == 0 {
				continue
			}
			value, err := decodeJSON(f.data)
			if err != nil {
				if err = write(nil, nil, &rpcError{-32700, "Parse error"}); err != nil {
					return err
				}
				continue
			}
			obj, ok := value.(map[string]any)
			var req request
			if !ok || json.Unmarshal(f.data, &req) != nil || req.JSONRPC != "2.0" || req.Method == "" {
				if err = write(nil, nil, &rpcError{-32600, "Invalid Request"}); err != nil {
					return err
				}
				continue
			}
			_, hasID := obj["id"]
			if hasID {
				switch id := obj["id"].(type) {
				case string:
				case json.Number:
					if _, err = id.Int64(); err != nil {
						hasID = false
					}
				default:
					hasID = false
				}
				if !hasID {
					if err = write(nil, nil, &rpcError{-32600, "Invalid request ID"}); err != nil {
						return err
					}
					continue
				}
			}
			if !hasID {
				if req.Method == "notifications/cancelled" {
					var p struct {
						RequestID json.RawMessage `json:"requestId"`
					}
					if json.Unmarshal(req.Params, &p) == nil {
						if a := active[idKey(p.RequestID)]; a != nil {
							a.cancelled = true
							a.cancel()
						}
					}
				}
				continue
			}
			key := idKey(req.ID)
			if _, exists := active[key]; exists {
				if err = write(req.ID, nil, &rpcError{-32600, "Request ID is already active"}); err != nil {
					return err
				}
				continue
			}
			var result any
			var rpcErr *rpcError
			switch req.Method {
			case "initialize":
				var p struct {
					ProtocolVersion string         `json:"protocolVersion"`
					Capabilities    map[string]any `json:"capabilities"`
					ClientInfo      map[string]any `json:"clientInfo"`
				}
				if initialized || json.Unmarshal(req.Params, &p) != nil || p.ProtocolVersion == "" || p.Capabilities == nil || p.ClientInfo == nil {
					rpcErr = &rpcError{-32602, "Invalid initialize parameters"}
					break
				}
				initialized = true
				result = map[string]any{"protocolVersion": protocolVersion, "capabilities": map[string]any{"tools": map[string]any{}}, "serverInfo": map[string]any{"name": "lane", "version": version}}
			case "ping":
				result = map[string]any{}
			case "tools/list":
				if !initialized {
					rpcErr = &rpcError{-32600, "Initialize first"}
					break
				}
				var p struct {
					Cursor string `json:"cursor"`
				}
				if len(req.Params) > 0 && (json.Unmarshal(req.Params, &p) != nil || p.Cursor != "") {
					rpcErr = &rpcError{-32602, "Invalid list parameters"}
					break
				}
				result = map[string]any{"tools": tools}
			case "tools/call":
				if !initialized {
					rpcErr = &rpcError{-32600, "Initialize first"}
					break
				}
				var p struct {
					Name      string          `json:"name"`
					Arguments json.RawMessage `json:"arguments"`
				}
				if json.Unmarshal(req.Params, &p) != nil {
					rpcErr = &rpcError{-32602, "Invalid call parameters"}
					break
				}
				tool, ok := catalog[p.Name]
				if !ok {
					rpcErr = &rpcError{-32602, "Unknown tool"}
					break
				}
				if len(p.Arguments) == 0 {
					p.Arguments = json.RawMessage("{}")
				}
				if err := tool.check(p.Arguments); err != nil {
					rpcErr = &rpcError{-32602, "Invalid tool arguments: " + err.Error()}
					break
				}
				if len(active) >= maxConcurrent {
					rpcErr = &rpcError{-32000, "Too many concurrent requests"}
					break
				}
				callCtx, callCancel := context.WithCancel(ctx)
				active[key] = &pending{id: req.ID, cancel: callCancel}
				go func() {
					r, e := call(callCtx, invoke, tool.method, p.Arguments)
					select {
					case done <- completion{key, r, e}:
					case <-ctx.Done():
					}
				}()
				continue
			default:
				rpcErr = &rpcError{-32601, "Method not found"}
			}
			if err = write(req.ID, result, rpcErr); err != nil {
				return err
			}
		}
	}
}

func call(ctx context.Context, invoke Invoke, method string, args json.RawMessage) (result any, rpcErr *rpcError) {
	defer func() {
		if recover() != nil {
			result = nil
			rpcErr = &rpcError{-32603, "Internal error"}
		}
	}()
	value, err := invoke(ctx, method, args)
	if err != nil {
		var detail interface{ ErrorDetails() any }
		if errors.As(err, &detail) {
			return toolResult(map[string]any{"error": detail.ErrorDetails()}, true), nil
		}
		return toolResult(map[string]any{"error": map[string]any{"message": err.Error()}}, true), nil
	}
	var raw []byte
	if message, ok := value.(proto.Message); ok {
		raw, err = protojson.Marshal(message)
	} else {
		raw, err = json.Marshal(value)
	}
	if err != nil {
		return nil, &rpcError{-32603, "Cannot encode tool result"}
	}
	decoded, err := decodeJSON(raw)
	if err != nil {
		return nil, &rpcError{-32603, "Cannot encode tool result"}
	}
	obj, ok := decoded.(map[string]any)
	if !ok {
		return nil, &rpcError{-32603, "Tool result must be an object"}
	}
	return toolResult(obj, false), nil
}

func toolResult(value map[string]any, failed bool) any {
	raw, _ := json.Marshal(value)
	return map[string]any{"structuredContent": value, "content": []any{map[string]any{"type": "text", "text": string(raw)}}, "isError": failed}
}

// Decode tokens rather than maps alone so duplicate keys cannot hide an origin
// field or make schema validation disagree with the downstream decoder.
func decodeJSON(raw []byte) (any, error) {
	d := json.NewDecoder(bytes.NewReader(raw))
	d.UseNumber()
	var read func(int) (any, error)
	read = func(depth int) (any, error) {
		if depth > 100 {
			return nil, errors.New("JSON nesting limit exceeded")
		}
		t, err := d.Token()
		if err != nil {
			return nil, err
		}
		delim, ok := t.(json.Delim)
		if !ok {
			return t, nil
		}
		switch delim {
		case '{':
			o := map[string]any{}
			for d.More() {
				k, err := d.Token()
				if err != nil {
					return nil, err
				}
				key, ok := k.(string)
				if !ok {
					return nil, errors.New("invalid object key")
				}
				if _, exists := o[key]; exists {
					return nil, errors.New("duplicate object key")
				}
				v, err := read(depth + 1)
				if err != nil {
					return nil, err
				}
				o[key] = v
			}
			_, err = d.Token()
			return o, err
		case '[':
			a := []any{}
			for d.More() {
				v, err := read(depth + 1)
				if err != nil {
					return nil, err
				}
				a = append(a, v)
			}
			_, err = d.Token()
			return a, err
		default:
			return nil, fmt.Errorf("unexpected JSON delimiter")
		}
	}
	v, err := read(0)
	if err != nil {
		return nil, err
	}
	if _, err = d.Token(); err != io.EOF {
		return nil, errors.New("trailing JSON data")
	}
	return v, nil
}

// IDs compare by decoded value, not by the spelling of JSON string escapes.
func idKey(raw json.RawMessage) string {
	value, err := decodeJSON(raw)
	if err != nil {
		return ""
	}
	canonical, err := json.Marshal(value)
	if err != nil {
		return ""
	}
	return string(canonical)
}
