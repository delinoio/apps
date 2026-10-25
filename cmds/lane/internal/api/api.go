// SPDX-License-Identifier: Apache-2.0
package api

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"strings"

	"connectrpc.com/connect"
	v1 "github.com/delinoio/apps/protos/lane/v1"
	"github.com/google/uuid"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/reflect/protoregistry"
)

func Descriptor(method string) protoreflect.MethodDescriptor {
	services := v1.File_lane_v1_lane_proto.Services()
	for i := 0; i < services.Len(); i++ {
		s := services.Get(i)
		if s.Name() == "SyncService" {
			continue
		}
		if m := s.Methods().ByName(protoreflect.Name(method)); m != nil {
			return m
		}
	}
	return nil
}

func Decode(method string, raw []byte) (proto.Message, error) {
	d := Descriptor(method)
	if d == nil {
		return nil, fmt.Errorf("unknown method %s", method)
	}
	t, err := protoregistry.GlobalTypes.FindMessageByName(d.Input().FullName())
	if err != nil {
		return nil, err
	}
	m := t.New().Interface()
	if err = protojson.Unmarshal(raw, m); err != nil {
		return nil, errors.New("invalid typed request")
	}
	return m, nil
}

// Attribute assigns identities at the adapter boundary, including nested event tasks.
func Attribute(m proto.Message, origin *v1.Origin) ([]string, error) {
	var allocated []string
	var visit func(protoreflect.Message) error
	visit = func(msg protoreflect.Message) error {
		fields := msg.Descriptor().Fields()
		if f := fields.ByName("meta"); f != nil && f.Message() != nil && f.Message().FullName() == "lane.v1.MutationMeta" {
			meta := msg.Mutable(f).Message().Interface().(*v1.MutationMeta)
			if meta.RequestId == "" {
				id, err := uuid.NewV7()
				if err != nil {
					return err
				}
				meta.RequestId = id.String()
				allocated = append(allocated, meta.RequestId)
			}
			id, err := uuid.Parse(meta.RequestId)
			if err != nil || id.Version() != 7 || id.String() != meta.RequestId {
				return errors.New("request ID must be a lowercase UUID v7")
			}
			meta.Origin = proto.Clone(origin).(*v1.Origin)
		}
		var result error
		msg.Range(func(f protoreflect.FieldDescriptor, v protoreflect.Value) bool {
			if f.Kind() != protoreflect.MessageKind || f.Name() == "meta" {
				return true
			}
			if f.IsList() {
				l := v.List()
				for i := 0; i < l.Len(); i++ {
					if result = visit(l.Get(i).Message()); result != nil {
						return false
					}
				}
			} else if !f.IsMap() {
				result = visit(v.Message())
			}
			return result == nil
		})
		return result
	}
	return allocated, visit(m.ProtoReflect())
}

type Failure struct {
	Reason         string          `json:"reason"`
	Message        string          `json:"message"`
	CorrelationID  string          `json:"correlationId"`
	Retryable      bool            `json:"retryable"`
	EntityID       string          `json:"entityId,omitempty"`
	CurrentHeadIDs []string        `json:"currentHeadIds,omitempty"`
	Conflict       json.RawMessage `json:"conflict,omitempty"`
}

func (f *Failure) Error() string { return f.Message }

// ErrorDetails exposes the same safe structured failure to CLI and MCP clients.
func (f *Failure) ErrorDetails() any { return f }
func Error(err error) *Failure {
	var f *Failure
	if errors.As(err, &f) {
		return f
	}
	id, _ := uuid.NewV7()
	f = &Failure{Reason: "ERROR_REASON_DEPENDENCY_UNAVAILABLE", Message: "Operation failed. Retry or check the local service.", CorrelationID: id.String()}
	var ce *connect.Error
	if errors.As(err, &ce) {
		for _, d := range ce.Details() {
			v, e := d.Value()
			if e == nil {
				if result, ok := v.(*v1.EntityResult); ok {
					f.Conflict, _ = protojson.Marshal(result)
				}
				if detail, ok := v.(*v1.ErrorDetail); ok {
					f.Reason = detail.Reason.String()
					f.CorrelationID = detail.CorrelationId
					f.Retryable = detail.Retryable
					f.EntityID = detail.EntityId
					f.CurrentHeadIDs = detail.CurrentHeadIds
				}
			}
		}
		if f.Reason == "ERROR_REASON_DEPENDENCY_UNAVAILABLE" {
			switch ce.Code() {
			case connect.CodeInvalidArgument:
				f.Reason = "ERROR_REASON_INVALID_INPUT"
			case connect.CodeUnauthenticated:
				f.Reason = "ERROR_REASON_UNAUTHENTICATED"
			case connect.CodePermissionDenied:
				f.Reason = "ERROR_REASON_FORBIDDEN"
			case connect.CodeNotFound:
				f.Reason = "ERROR_REASON_NOT_FOUND"
			}
		}
	}
	// Error messages can contain SQL parameters, filesystem paths, or remote bodies.
	f.Message = strings.ReplaceAll(strings.TrimPrefix(f.Reason, "ERROR_REASON_"), "_", " ") + "."
	return f
}
func JSON(v any) (json.RawMessage, error) {
	if p, ok := v.(proto.Message); ok {
		return protojson.MarshalOptions{EmitDefaultValues: true}.Marshal(p)
	}
	return json.Marshal(v)
}

// AuthTransport attaches a credential to the explicitly selected endpoint only.
type AuthTransport struct {
	Base  http.RoundTripper
	Token string
}

func (t AuthTransport) RoundTrip(r *http.Request) (*http.Response, error) {
	c := r.Clone(r.Context())
	c.Header = r.Header.Clone()
	c.Header.Set("Authorization", "Bearer "+t.Token)
	b := t.Base
	if b == nil {
		b = http.DefaultTransport
	}
	return b.RoundTrip(c)
}

type Invoker func(context.Context, string, proto.Message) (proto.Message, error)
