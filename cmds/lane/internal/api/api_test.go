// SPDX-License-Identifier: Apache-2.0
package api

import (
	"connectrpc.com/connect"
	"context"
	v1 "github.com/delinoio/apps/protos/lane/v1"
	"github.com/delinoio/apps/protos/lane/v1/lanev1connect"
	"net/http"
	"net/http/httptest"
	"testing"
)

type handler struct {
	lanev1connect.UnimplementedLaneServiceHandler
	t *testing.T
}

func (h handler) GetTask(ctx context.Context, r *connect.Request[v1.GetTaskRequest]) (*connect.Response[v1.GetTaskResponse], error) {
	if r.Msg.Id != "id" || r.Header().Get("Authorization") != "Bearer fake" {
		h.t.Fatal(r.Msg, r.Header())
	}
	return connect.NewResponse(&v1.GetTaskResponse{Result: &v1.EntityResult{Result: v1.ResultKind_RESULT_KIND_DELETED}}), nil
}
func TestTypedRemote(t *testing.T) {
	p, h := lanev1connect.NewLaneServiceHandler(handler{t: t})
	mux := http.NewServeMux()
	mux.Handle(p, h)
	s := httptest.NewServer(mux)
	defer s.Close()
	fn := Remote(&http.Client{Transport: AuthTransport{Token: "fake"}}, s.URL)
	r, e := fn(context.Background(), "GetTask", &v1.GetTaskRequest{Id: "id"})
	if e != nil || r.(*v1.GetTaskResponse).Result.Result != v1.ResultKind_RESULT_KIND_DELETED {
		t.Fatal(r, e)
	}
}
func TestNestedOriginAttribution(t *testing.T) {
	m := &v1.CompleteEventRequest{Tasks: []*v1.CreateTaskRequest{{Meta: &v1.MutationMeta{Origin: &v1.Origin{AccountId: "spoof"}}}}}
	ids, e := Attribute(m, &v1.Origin{Kind: v1.OriginKind_ORIGIN_KIND_MCP, DeviceId: "real", AccountId: "account"})
	if e != nil || len(ids) != 2 || m.Tasks[0].Meta.Origin.AccountId != "account" || m.Meta.Origin.DeviceId != "real" {
		t.Fatal(m, ids, e)
	}
}
func TestDecodeRejectsUnknownFields(t *testing.T) {
	if _, e := Decode("CreateTask", []byte(`{"surprise":true}`)); e == nil {
		t.Fatal("unknown field accepted")
	}
}
