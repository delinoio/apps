// SPDX-License-Identifier: FSL-1.1-ALv2

package server

import (
	"bytes"
	"encoding/json"
	"errors"

	"connectrpc.com/connect"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/encoding/protowire"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
)

// Codec names that connect-go registers by default for handlers. Replacing
// them keeps every wire format on the same decode-failure handling.
const (
	codecNameProto           = "proto"
	codecNameJSON            = "json"
	codecNameJSONCharsetUTF8 = "json; charset=utf-8"
)

// invalidInputMessage is the fixed caller-visible message of a request that
// could not be decoded. It never includes request content.
const invalidInputMessage = "request body is not a valid message"

// decodeFailureField is the unknown-field number that marks a request whose
// body failed to decode. It is the largest legal field number, which no
// mcpa.v1 message uses.
const decodeFailureField protowire.Number = protowire.MaxValidNumber

// decodeFailureMarker is the only unknown field a decode failure leaves on the
// request message.
var decodeFailureMarker = protowire.AppendBytes(protowire.AppendTag(nil, decodeFailureField, protowire.BytesType), []byte("decode_failed"))

// requestDecodeFailed reports whether the codecs of this package rejected the
// request body.
//
// Workaround: connect-go decodes a unary request before any interceptor runs
// and wraps the codec error, which quotes the offending text (for example
// `unmarshal into *mcpav1.X: proto: syntax error ... "sk-..."`), in an
// untyped invalid_argument error with no ErrorDetail. A codec cannot return a
// typed error because connect-go builds the outer error itself, and there is
// no per-request hook between decoding and the interceptor. The codecs
// therefore swallow the decode error, reset the message and leave this
// in-band marker, so that the management interceptor renders a fixed typed
// error through the normal path. Remove it when connect-go lets a codec or
// handler option shape decode errors.
func requestDecodeFailed(msg any) bool {
	m, ok := msg.(proto.Message)
	if !ok {
		return false
	}
	return bytes.Equal(m.ProtoReflect().GetUnknown(), decodeFailureMarker)
}

func markDecodeFailed(msg proto.Message) {
	r := msg.ProtoReflect()
	proto.Reset(msg)
	r.SetUnknown(protoreflect.RawFields(decodeFailureMarker))
}

var errNotProto = errors.New("message is not a protobuf message")

// handlerCodecs returns the connect handler options that install the
// content-free decode failure behavior for every default codec.
func handlerCodecs() []connect.HandlerOption {
	return []connect.HandlerOption{
		connect.WithCodec(&jsonCodec{name: codecNameJSON}),
		connect.WithCodec(&jsonCodec{name: codecNameJSONCharsetUTF8}),
		connect.WithCodec(binaryCodec{}),
	}
}

// binaryCodec is the binary protobuf codec.
type binaryCodec struct{}

func (binaryCodec) Name() string { return codecNameProto }

func (binaryCodec) Marshal(message any) ([]byte, error) {
	m, ok := message.(proto.Message)
	if !ok {
		return nil, errNotProto
	}
	return proto.Marshal(m)
}

func (binaryCodec) MarshalAppend(dst []byte, message any) ([]byte, error) {
	m, ok := message.(proto.Message)
	if !ok {
		return nil, errNotProto
	}
	return proto.MarshalOptions{}.MarshalAppend(dst, m)
}

func (binaryCodec) MarshalStable(message any) ([]byte, error) {
	m, ok := message.(proto.Message)
	if !ok {
		return nil, errNotProto
	}
	return proto.MarshalOptions{Deterministic: true}.Marshal(m)
}

func (binaryCodec) IsBinary() bool { return true }

func (binaryCodec) Unmarshal(data []byte, message any) error {
	m, ok := message.(proto.Message)
	if !ok {
		return errNotProto
	}
	if err := proto.Unmarshal(data, m); err != nil {
		markDecodeFailed(m)
	}
	return nil
}

// jsonCodec is the ProtoJSON codec.
type jsonCodec struct{ name string }

func (c *jsonCodec) Name() string { return c.name }

func (c *jsonCodec) Marshal(message any) ([]byte, error) {
	m, ok := message.(proto.Message)
	if !ok {
		return nil, errNotProto
	}
	return protojson.MarshalOptions{}.Marshal(m)
}

func (c *jsonCodec) MarshalAppend(dst []byte, message any) ([]byte, error) {
	m, ok := message.(proto.Message)
	if !ok {
		return nil, errNotProto
	}
	return protojson.MarshalOptions{}.MarshalAppend(dst, m)
}

func (c *jsonCodec) MarshalStable(message any) ([]byte, error) {
	b, err := c.Marshal(message)
	if err != nil {
		return nil, err
	}
	var out bytes.Buffer
	if err := json.Compact(&out, b); err != nil {
		return nil, err
	}
	return out.Bytes(), nil
}

func (c *jsonCodec) IsBinary() bool { return false }

func (c *jsonCodec) Unmarshal(data []byte, message any) error {
	m, ok := message.(proto.Message)
	if !ok {
		return errNotProto
	}
	// Match connect-go: unknown fields are discarded so that clients and
	// servers need not share one schema version.
	if len(data) == 0 || (protojson.UnmarshalOptions{DiscardUnknown: true}).Unmarshal(data, m) != nil {
		markDecodeFailed(m)
	}
	return nil
}
