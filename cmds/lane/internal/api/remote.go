// SPDX-License-Identifier: Apache-2.0
package api

import (
	"connectrpc.com/connect"
	"context"
	"errors"
	v1 "github.com/delinoio/apps/protos/lane/v1"
	"github.com/delinoio/apps/protos/lane/v1/lanev1connect"
	"google.golang.org/protobuf/proto"
	"net/http"
)

func Remote(client *http.Client, endpoint string) Invoker {
	laneService := lanev1connect.NewLaneServiceClient(client, endpoint)
	accountService := lanev1connect.NewAccountServiceClient(client, endpoint)
	webhookService := lanev1connect.NewWebhookServiceClient(client, endpoint)
	return func(ctx context.Context, method string, m proto.Message) (proto.Message, error) {
		switch method {
		case "GetRepository":
			q, ok := m.(*v1.GetRepositoryRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.GetRepository(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "ListRepositories":
			q, ok := m.(*v1.ListRepositoriesRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.ListRepositories(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "CreateRepository":
			q, ok := m.(*v1.CreateRepositoryRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.CreateRepository(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "SaveRepository":
			q, ok := m.(*v1.SaveRepositoryRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.SaveRepository(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "DeleteRepository":
			q, ok := m.(*v1.DeleteRepositoryRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.DeleteRepository(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "GetTask":
			q, ok := m.(*v1.GetTaskRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.GetTask(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "ListTasks":
			q, ok := m.(*v1.ListTasksRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.ListTasks(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "CreateTask":
			q, ok := m.(*v1.CreateTaskRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.CreateTask(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "SaveTask":
			q, ok := m.(*v1.SaveTaskRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.SaveTask(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "SetProgress":
			q, ok := m.(*v1.SetProgressRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.SetProgress(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "SetProposal":
			q, ok := m.(*v1.SetProposalRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.SetProposal(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "RestorePrompt":
			q, ok := m.(*v1.RestorePromptRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.RestorePrompt(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "DeleteTask":
			q, ok := m.(*v1.DeleteTaskRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.DeleteTask(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "ListHistory":
			q, ok := m.(*v1.ListHistoryRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.ListHistory(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "ResolveConflict":
			q, ok := m.(*v1.ResolveConflictRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := laneService.ResolveConflict(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "ListDevices":
			q, ok := m.(*v1.ListDevicesRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := accountService.ListDevices(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "DisconnectDevice":
			q, ok := m.(*v1.DisconnectDeviceRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := accountService.DisconnectDevice(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "ListConnections":
			q, ok := m.(*v1.ListConnectionsRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := accountService.ListConnections(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "RevokeConnection":
			q, ok := m.(*v1.RevokeConnectionRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := accountService.RevokeConnection(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "RegisterPush":
			q, ok := m.(*v1.RegisterPushRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := accountService.RegisterPush(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "BeginDataDeletion":
			q, ok := m.(*v1.BeginDataDeletionRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := accountService.BeginDataDeletion(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "DeleteAccountData":
			q, ok := m.(*v1.DeleteAccountDataRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := accountService.DeleteAccountData(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "ListReceivers":
			q, ok := m.(*v1.ListReceiversRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.ListReceivers(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "CreateReceiver":
			q, ok := m.(*v1.CreateReceiverRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.CreateReceiver(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "SaveReceiver":
			q, ok := m.(*v1.SaveReceiverRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.SaveReceiver(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "RotateReceiverKey":
			q, ok := m.(*v1.RotateReceiverKeyRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.RotateReceiverKey(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "DeleteReceiver":
			q, ok := m.(*v1.DeleteReceiverRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.DeleteReceiver(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "ListEvents":
			q, ok := m.(*v1.ListEventsRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.ListEvents(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "GetEvent":
			q, ok := m.(*v1.GetEventRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.GetEvent(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "DeleteEvent":
			q, ok := m.(*v1.DeleteEventRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.DeleteEvent(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "ClaimEvent":
			q, ok := m.(*v1.ClaimEventRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.ClaimEvent(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "RenewEventLease":
			q, ok := m.(*v1.RenewEventLeaseRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.RenewEventLease(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "ReleaseEventLease":
			q, ok := m.(*v1.ReleaseEventLeaseRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.ReleaseEventLease(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "CompleteEvent":
			q, ok := m.(*v1.CompleteEventRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.CompleteEvent(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		case "ReopenEvent":
			q, ok := m.(*v1.ReopenEventRequest)
			if !ok {
				return nil, errors.New("invalid request type")
			}
			r, e := webhookService.ReopenEvent(ctx, connect.NewRequest(q))
			if e != nil {
				return nil, e
			}
			return r.Msg, nil
		}
		return nil, errors.New("unknown method")
	}
}
