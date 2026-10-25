// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"os"
	"reflect"
	"sort"
	"testing"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
)

func TestSharedSyncApplyVectors(t *testing.T) {
	raw, e := os.ReadFile("../../../../protos/lane/v1/fixtures/sync.json")
	if e != nil {
		t.Fatal(e)
	}
	var fixture struct {
		Cases []struct {
			ID    string `json:"id"`
			Input struct {
				Initial   []json.RawMessage `json:"initial"`
				Mutations []json.RawMessage `json:"mutations"`
				Version   json.RawMessage   `json:"version"`
			} `json:"input"`
			Expected struct {
				HeadIDs []string `json:"headIds"`
			} `json:"expected"`
		} `json:"cases"`
	}
	if e = json.Unmarshal(raw, &fixture); e != nil {
		t.Fatal(e)
	}
	selected := map[string]bool{"whole-task-conflict": true, "ancestor-replay": true, "edit-then-delete": true, "delete-then-edit": true, "delete-then-delayed-create": true, "immutable-version-id": true, "unsupported-format": true}
	for _, tc := range fixture.Cases {
		if !selected[tc.ID] {
			continue
		}
		delete(selected, tc.ID)
		t.Run(tc.ID, func(t *testing.T) {
			s := openTestStore(t)
			apply := func(v *lanev1.EntityVersion) error {
				return s.WithWrite(context.Background(), func(c *sql.Conn) error {
					return ApplySyncResult(context.Background(), c, &lanev1.EntityResult{Version: v})
				})
			}
			for _, body := range tc.Input.Initial {
				v := new(lanev1.EntityVersion)
				if e := protojson.Unmarshal(body, v); e != nil {
					t.Fatal(e)
				}
				if e := apply(v); e != nil {
					t.Fatal(e)
				}
			}
			if tc.ID == "unsupported-format" {
				v := new(lanev1.EntityVersion)
				if e := protojson.Unmarshal(tc.Input.Version, v); e != nil {
					t.Fatal(e)
				}
				if e := apply(v); !errors.Is(e, ErrUnsupportedFormat) {
					t.Fatalf("format: %v", e)
				}
				return
			}
			rejected := false
			for _, body := range tc.Input.Mutations {
				m := new(lanev1.SyncMutation)
				if e := protojson.Unmarshal(body, m); e != nil {
					t.Fatal(e)
				}
				e := apply(m.Version)
				if e != nil {
					if tc.ID != "immutable-version-id" {
						t.Fatal(e)
					}
					rejected = true
				}
			}
			if tc.ID == "immutable-version-id" {
				if !rejected {
					t.Fatal("changed immutable version accepted")
				}
				return
			}
			var heads []string
			if e := s.WithRead(context.Background(), func(c *sql.Conn) error {
				var e error
				heads, e = stringRows(context.Background(), c, "SELECT version_id FROM heads ORDER BY version_id")
				return e
			}); e != nil {
				t.Fatal(e)
			}
			sort.Strings(tc.Expected.HeadIDs)
			if !reflect.DeepEqual(heads, tc.Expected.HeadIDs) {
				t.Fatalf("heads %v, expected %v", heads, tc.Expected.HeadIDs)
			}
		})
	}
	if len(selected) != 0 {
		t.Fatalf("missing shared vectors %v", selected)
	}
}
func TestStrictEnvelopeValidation(t *testing.T) {
	good := version("task", "v")
	cases := []func(*lanev1.EntityVersion){func(v *lanev1.EntityVersion) { v.EntityId = "not-uuid" }, func(v *lanev1.EntityVersion) { v.GetTask().Progress = 0 }, func(v *lanev1.EntityVersion) { v.GetTask().Proposal = 0 }, func(v *lanev1.EntityVersion) { v.Origin.Kind = 0 }, func(v *lanev1.EntityVersion) { v.ParentVersionIds = []string{tid("b"), tid("b")} }, func(v *lanev1.EntityVersion) { v.Deleted = true; v.Body = nil; v.PromptRestoration = true }}
	for i, change := range cases {
		v := proto.Clone(good).(*lanev1.EntityVersion)
		change(v)
		if e := validateVersion(v); e == nil {
			t.Fatalf("invalid envelope %d accepted", i)
		}
	}
}
func TestDeletedRepositoryRejectsDelayedTask(t *testing.T) {
	s := openTestStore(t)
	tomb := &lanev1.EntityVersion{FormatVersion: 1, EntityId: tid("repo"), VersionId: tid("repo-delete"), Kind: lanev1.EntityKind_ENTITY_KIND_REPOSITORY, Deleted: true}
	if e := s.WithWrite(context.Background(), func(c *sql.Conn) error {
		if e := ApplySyncResult(context.Background(), c, &lanev1.EntityResult{Version: tomb}); e != nil {
			return e
		}
		return ApplySyncResult(context.Background(), c, &lanev1.EntityResult{Version: version("late", "v")})
	}); e != nil {
		t.Fatal(e)
	}
	if count(t, s, "SELECT count(*) FROM entities WHERE kind=2 AND deleted=1") != 1 || count(t, s, "SELECT count(*) FROM versions WHERE envelope_json LIKE '%private title%'") != 0 {
		t.Fatal("delayed task resurrected")
	}
}
func TestPromptRestorationMarkerSurvivesSync(t *testing.T) {
	s := openTestStore(t)
	base, next := version("task", "base"), version("task", "next", "base")
	next.PromptRestoration = true
	for _, v := range []*lanev1.EntityVersion{base, next} {
		if e := s.WithWrite(context.Background(), func(c *sql.Conn) error {
			return ApplySyncResult(context.Background(), c, &lanev1.EntityResult{Version: v})
		}); e != nil {
			t.Fatal(e)
		}
	}
	var marker int
	if e := s.db.QueryRow("SELECT prompt_version FROM versions WHERE version_id=?", next.VersionId).Scan(&marker); e != nil || marker != 1 {
		t.Fatalf("restoration absent marker=%d err=%v", marker, e)
	}
}
