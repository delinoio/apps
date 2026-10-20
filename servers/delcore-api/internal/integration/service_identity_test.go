// SPDX-License-Identifier: FSL-1.1-ALv2
//go:build integration

package integration

import (
	"connectrpc.com/connect"
	"context"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"testing"
)

func TestDeliToolsServiceIdentityCannotMove(t *testing.T) {
	_, client, _ := setup(t)
	put := func(id, name, clientID string) (*connect.Response[pb.PutServiceResponse], error) {
		return client.PutService(context.Background(), req(&pb.PutServiceRequest{RequestKey: key(), Reason: "verify reserved service identity", Service: &pb.Service{Id: id, Name: name, ClientId: clientID, Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	}
	reserved, err := put("", "delitools", "service")
	if err != nil {
		t.Fatal(err)
	}
	other, err := put("", "other", "other-client")
	if err != nil {
		t.Fatal(err)
	}
	for _, change := range []struct{ id, name, client string }{
		{reserved.Msg.Service.Id, "renamed", "service"},
		{other.Msg.Service.Id, "delitools", "other-client"},
	} {
		if _, err := put(change.id, change.name, change.client); connect.CodeOf(err) != connect.CodeFailedPrecondition {
			t.Fatalf("identity reassignment accepted: %v", err)
		}
	}
	if _, err := put(reserved.Msg.Service.Id, "delitools", "updated-client"); err != nil {
		t.Fatal(err)
	}
	if _, err := put(other.Msg.Service.Id, "other-renamed", "other-client"); err != nil {
		t.Fatal(err)
	}
}
