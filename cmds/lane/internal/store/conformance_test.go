// SPDX-License-Identifier: Apache-2.0
package store

import (
	"connectrpc.com/connect"
	"context"
	"database/sql"
	"encoding/json"
	lane "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"os"
	"testing"
)

const fixtureRepo = "019a0000-0000-7000-8000-000000000002"

func fixtureTask(t *testing.T, s *Store, raw json.RawMessage) *lane.EntityVersion {
	t.Helper()
	task := new(lane.Task)
	if e := protojson.Unmarshal(raw, task); e != nil {
		t.Fatal(e)
	}
	if _, e := s.CreateRepository(ctx, connect.NewRequest(&lane.CreateRepositoryRequest{Meta: meta(s), Id: fixtureRepo, Url: "https://example.com/fixture"})); e != nil {
		t.Fatal(e)
	}
	r, e := s.CreateTask(ctx, connect.NewRequest(&lane.CreateTaskRequest{Meta: meta(s), Repository: &lane.RepositoryTarget{Target: &lane.RepositoryTarget_Id{Id: task.RepositoryId}}, Title: task.Title, Description: task.Description, Prompt: task.Prompt, Progress: task.Progress, Proposal: task.Proposal}))
	if e != nil {
		t.Fatal(e)
	}
	return r.Msg.Result.Version
}
func TestSharedTaskVectors(t *testing.T) {
	var f struct {
		Cases []struct {
			ID    string
			Input struct {
				Method, Operation, Query, Title, Description string
				Request, Before, Task                        json.RawMessage
				Decisions                                    []string
				Snapshots                                    []json.RawMessage
				RestoreSourceIndex                           int
			}
			Expected struct {
				Task                                                                                                    json.RawMessage
				Reason, Progress, Proposal, Prompt                                                                      string
				PromptPresent                                                                                           bool
				RepositoryCount, TaskCount, HistoryCount, NewVersions, NewVersionsIncludingCreation, PromptHistoryCount int
				Matches                                                                                                 bool
				Progresses, Proposals                                                                                   []string
			}
		}
	}
	b, e := os.ReadFile("../../../../protos/lane/v1/fixtures/tasks.json")
	if e != nil {
		t.Fatal(e)
	}
	if e = json.Unmarshal(b, &f); e != nil {
		t.Fatal(e)
	}
	for _, tc := range f.Cases {
		t.Run(tc.ID, func(t *testing.T) {
			s := testStore(t)
			in, want := tc.Input, tc.Expected
			switch {
			case in.Method == "CreateTask":
				r := new(lane.CreateTaskRequest)
				if e := protojson.Unmarshal(in.Request, r); e != nil {
					t.Fatal(e)
				}
				r.Meta.Origin.DeviceId = s.DeviceID()
				got, e := s.CreateTask(ctx, connect.NewRequest(r))
				if want.Reason != "" {
					if reason(e).String() != want.Reason || count(t, s, "entities") != 0 {
						t.Fatalf("invalid create wrote: %v", e)
					}
					return
				}
				if e != nil {
					t.Fatal(e)
				}
				task := got.Msg.Result.Version.GetTask()
				if task.Progress.String() != want.Progress || task.Proposal.String() != want.Proposal || (task.Prompt != nil) != want.PromptPresent {
					t.Fatal("creation defaults differ")
				}
			case in.Method == "SetProgress":
				v := fixtureTask(t, s, in.Before)
				r := new(lane.SetProgressRequest)
				if e := protojson.Unmarshal(in.Request, r); e != nil {
					t.Fatal(e)
				}
				r.Meta.Origin.DeviceId = s.DeviceID()
				r.Id = v.EntityId
				r.ExpectedVersionId = v.VersionId
				got, e := s.SetProgress(ctx, connect.NewRequest(r))
				if e != nil {
					t.Fatal(e)
				}
				task := new(lane.Task)
				if e = protojson.Unmarshal(want.Task, task); e != nil {
					t.Fatal(e)
				}
				if !proto.Equal(task, got.Msg.Result.Version.GetTask()) {
					t.Fatal("transition changed unrelated fields")
				}
				if count(t, s, "versions") != 2+want.NewVersions {
					t.Fatal("version missing")
				}
			case in.Method == "SaveTask":
				v := fixtureTask(t, s, in.Before)
				r := new(lane.SaveTaskRequest)
				if e := protojson.Unmarshal(in.Request, r); e != nil {
					t.Fatal(e)
				}
				r.Meta.Origin.DeviceId = s.DeviceID()
				r.Id = v.EntityId
				r.ExpectedVersionId = v.VersionId
				_, e := s.SaveTask(ctx, connect.NewRequest(r))
				if reason(e).String() != want.Reason || count(t, s, "versions") != 2 {
					t.Fatal("invalid proposal update", e)
				}
			case len(in.Decisions) > 0:
				v := fixtureTask(t, s, in.Task)
				for i, p := range in.Decisions {
					r, e := s.SetProposal(ctx, connect.NewRequest(&lane.SetProposalRequest{Meta: meta(s), Id: v.EntityId, ExpectedVersionId: v.VersionId, Proposal: lane.Proposal(lane.Proposal_value[p])}))
					if e != nil {
						t.Fatal(e)
					}
					v = r.Msg.Result.Version
					if v.GetTask().Progress.String() != want.Progresses[i] || v.GetTask().Proposal.String() != want.Proposals[i] {
						t.Fatal("proposal changes differ")
					}
				}
			case len(in.Snapshots) > 0:
				v := fixtureTask(t, s, in.Snapshots[0])
				ids := []string{v.VersionId}
				for _, raw := range in.Snapshots[1:] {
					task := new(lane.Task)
					if e := protojson.Unmarshal(raw, task); e != nil {
						t.Fatal(e)
					}
					r, e := s.SaveTask(ctx, connect.NewRequest(&lane.SaveTaskRequest{Meta: meta(s), Id: v.EntityId, ExpectedVersionId: v.VersionId, Task: task}))
					if e != nil {
						t.Fatal(e)
					}
					v = r.Msg.Result.Version
					ids = append(ids, v.VersionId)
				}
				r, e := s.RestorePrompt(ctx, connect.NewRequest(&lane.RestorePromptRequest{Meta: meta(s), Id: v.EntityId, ExpectedVersionId: v.VersionId, SourceVersionId: ids[in.RestoreSourceIndex]}))
				if e != nil {
					t.Fatal(e)
				}
				if !r.Msg.Result.Version.PromptRestoration || r.Msg.Result.Version.GetTask().Prompt == nil || r.Msg.Result.Version.GetTask().GetPrompt() != want.Prompt || count(t, s, "versions") != want.NewVersionsIncludingCreation+1 {
					t.Fatal("restoration differs")
				}
				if want.PromptHistoryCount > 0 {
					h, e := s.ListHistory(ctx, connect.NewRequest(&lane.ListHistoryRequest{Id: v.EntityId, PromptsOnly: true}))
					if e != nil || len(h.Msg.Versions) != want.PromptHistoryCount {
						t.Fatal("prompt history differs", e)
					}
				}
			case in.Operation == "validateStoredTask":
				task := new(lane.Task)
				if e := protojson.Unmarshal(in.Task, task); e != nil {
					t.Fatal(e)
				}
				e := s.WithWrite(context.Background(), func(c *sql.Conn) error { return validateTask(ctx, c, task) })
				if reason(e).String() != want.Reason {
					t.Fatal("invalid stored enum", e)
				}
			case in.Operation == "search":
				if MatchesSearch(in.Title, in.Description, in.Query) != want.Matches {
					t.Fatal("search differs")
				}
				if in.Title == "" {
					return
				}
				r := &lane.CreateTaskRequest{Meta: meta(s), Title: in.Title, Description: in.Description, Repository: &lane.RepositoryTarget{Target: &lane.RepositoryTarget_Url{Url: "https://example.com/search"}}}
				if _, e := s.CreateTask(ctx, connect.NewRequest(r)); e != nil {
					t.Fatal(e)
				}
				got, e := s.ListTasks(ctx, connect.NewRequest(&lane.ListTasksRequest{Search: in.Query}))
				if e != nil || (len(got.Msg.Items) > 0) != want.Matches {
					t.Fatal("search differs", e)
				}
			default:
				t.Fatalf("unhandled shared vector %s", tc.ID)
			}
		})
	}
}
