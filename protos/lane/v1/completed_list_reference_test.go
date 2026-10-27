// SPDX-License-Identifier: Apache-2.0
package lanev1_test

import (
	"encoding/json"
	"fmt"
	"os"
	"slices"
	"sort"
	"strings"
	"testing"
	"time"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"
)

type completedView struct {
	SelectedVersionID          string   `json:"selectedVersionId"`
	HeadVersionIDs             []string `json:"headVersionIds"`
	DisplayedHistoryVersionIDs []string `json:"displayedHistoryVersionIds"`
}

type completedEntryCase struct {
	ID       string          `json:"id"`
	Input    completedView   `json:"input"`
	Expected json.RawMessage `json:"expected"`
}

type completedListCase struct {
	ID    string `json:"id"`
	Input struct {
		Request json.RawMessage `json:"request"`
	} `json:"input"`
	Expected struct {
		TaskIDs []string   `json:"taskIds"`
		Pages   [][]string `json:"pages"`
	} `json:"expected"`
}

type completedSuite struct {
	FormatVersion int    `json:"formatVersion"`
	QueryInstant  string `json:"queryInstant"`
	Dataset       struct {
		Versions []json.RawMessage `json:"versions"`
		Rows     []completedView   `json:"rows"`
	} `json:"dataset"`
	EntryCases   []completedEntryCase `json:"entryCases"`
	ListCases    []completedListCase  `json:"listCases"`
	InvalidCases []struct {
		ID      string          `json:"id"`
		Request json.RawMessage `json:"request"`
	} `json:"invalidCases"`
	CursorCases []struct {
		ID              string          `json:"id"`
		First           json.RawMessage `json:"first"`
		Resume          json.RawMessage `json:"resume"`
		FirstAccountID  string          `json:"firstAccountId"`
		ResumeAccountID string          `json:"resumeAccountId"`
		FirstSpaceID    string          `json:"firstSpaceId"`
		ResumeSpaceID   string          `json:"resumeSpaceId"`
		Valid           bool            `json:"valid"`
	} `json:"cursorCases"`
}

type entryCandidate struct {
	transitionID string
	at           *timestamppb.Timestamp
}

type completedReference struct {
	versions map[string]*lanev1.EntityVersion
	rows     []completedView
}

func (m *completedReference) version(id string) *lanev1.EntityVersion {
	v := m.versions[id]
	if v == nil || v.GetTask() == nil || v.GetCreatedAt() == nil || v.CreatedAt.CheckValid() != nil {
		return nil
	}
	return v
}

func terminalProgress(p lanev1.Progress) bool {
	return p == lanev1.Progress_PROGRESS_DONE || p == lanev1.Progress_PROGRESS_CANCELED
}

// entryCandidateFor follows declared parents. A candidate is unique only when
// every ancestry path identifies the same transition version, not merely the same time.
func (m *completedReference) entryCandidateFor(id string, visiting map[string]bool) *entryCandidate {
	v := m.version(id)
	if v == nil || !terminalProgress(v.GetTask().GetProgress()) || visiting[id] {
		return nil
	}
	if len(v.ParentVersionIds) == 0 {
		return &entryCandidate{transitionID: id, at: v.CreatedAt}
	}
	visiting[id] = true
	defer delete(visiting, id)
	var result *entryCandidate
	for _, parentID := range v.ParentVersionIds {
		parent := m.version(parentID)
		if parent == nil || parent.EntityId != v.EntityId {
			return nil
		}
		candidate := &entryCandidate{transitionID: id, at: v.CreatedAt}
		if parent.GetTask().GetProgress() == v.GetTask().GetProgress() {
			candidate = m.entryCandidateFor(parentID, visiting)
		}
		if candidate == nil || (result != nil && result.transitionID != candidate.transitionID) {
			return nil
		}
		result = candidate
	}
	return result
}

func (m *completedReference) terminalEntry(view completedView) *lanev1.TerminalEntry {
	selected := m.version(view.SelectedVersionID)
	if selected == nil || !terminalProgress(selected.GetTask().GetProgress()) {
		return nil
	}
	unknown := &lanev1.TerminalEntry{Kind: lanev1.TerminalEntryKind_TERMINAL_ENTRY_KIND_UNKNOWN}
	if len(view.HeadVersionIDs) == 0 || !slices.Contains(view.HeadVersionIDs, view.SelectedVersionID) {
		return unknown
	}
	var result *entryCandidate
	for _, headID := range view.HeadVersionIDs {
		head := m.version(headID)
		if head == nil || head.EntityId != selected.EntityId || head.GetTask().GetProgress() != selected.GetTask().GetProgress() {
			return unknown
		}
		candidate := m.entryCandidateFor(headID, map[string]bool{})
		if candidate == nil || (result != nil && result.transitionID != candidate.transitionID) {
			return unknown
		}
		result = candidate
	}
	if result == nil {
		return unknown
	}
	return &lanev1.TerminalEntry{Kind: lanev1.TerminalEntryKind_TERMINAL_ENTRY_KIND_KNOWN, EnteredAt: result.at}
}

func validCompletedFilter(filter *lanev1.CompletedFilter) bool {
	if filter == nil {
		return true
	}
	switch filter.Selection {
	case lanev1.CompletedSelection_COMPLETED_SELECTION_NONE, lanev1.CompletedSelection_COMPLETED_SELECTION_ALL:
		return filter.Cutoff == nil
	case lanev1.CompletedSelection_COMPLETED_SELECTION_LAST_24_HOURS,
		lanev1.CompletedSelection_COMPLETED_SELECTION_LAST_7_DAYS,
		lanev1.CompletedSelection_COMPLETED_SELECTION_LAST_30_DAYS:
		return filter.Cutoff != nil && filter.Cutoff.CheckValid() == nil && filter.Cutoff.Nanos%1000 == 0
	default:
		return false
	}
}

func (m *completedReference) completedMatch(view completedView, filter *lanev1.CompletedFilter) bool {
	if filter == nil {
		return true
	}
	selected := m.version(view.SelectedVersionID)
	if selected == nil {
		return false
	}
	if !terminalProgress(selected.GetTask().GetProgress()) {
		return true
	}
	switch filter.Selection {
	case lanev1.CompletedSelection_COMPLETED_SELECTION_NONE:
		return false
	case lanev1.CompletedSelection_COMPLETED_SELECTION_ALL:
		return true
	default:
		entry := m.terminalEntry(view)
		if entry.GetKind() == lanev1.TerminalEntryKind_TERMINAL_ENTRY_KIND_UNKNOWN {
			return true
		}
		return entry.GetEnteredAt().AsTime().Compare(filter.Cutoff.AsTime()) >= 0
	}
}

func (m *completedReference) list(req *lanev1.ListTasksRequest) ([]string, error) {
	if !validCompletedFilter(req.Completed) {
		return nil, fmt.Errorf("invalid completed filter")
	}
	var ids []string
	for _, view := range m.rows {
		v := m.version(view.SelectedVersionID)
		if v == nil {
			return nil, fmt.Errorf("missing selected version %s", view.SelectedVersionID)
		}
		task := v.GetTask()
		if req.RepositoryId != nil && task.RepositoryId != *req.RepositoryId {
			continue
		}
		if len(req.Progress) > 0 && !slices.Contains(req.Progress, task.Progress) {
			continue
		}
		if len(req.Proposal) > 0 && !slices.Contains(req.Proposal, task.Proposal) {
			continue
		}
		if req.Search != "" && !strings.Contains(strings.ToLower(task.Title), strings.ToLower(req.Search)) {
			continue
		}
		if req.Filter != nil && len(req.Filter.Priorities) > 0 && !slices.Contains(req.Filter.Priorities, task.Priority) {
			continue
		}
		if !m.completedMatch(view, req.Completed) {
			continue
		}
		ids = append(ids, v.EntityId)
	}
	if req.SortKey == lanev1.TaskSortKey_TASK_SORT_KEY_PRIORITY {
		priority := map[string]lanev1.TaskPriority{}
		for _, view := range m.rows {
			v := m.version(view.SelectedVersionID)
			priority[v.EntityId] = v.GetTask().Priority
		}
		sort.Slice(ids, func(i, j int) bool {
			a, b := priority[ids[i]], priority[ids[j]]
			return compareTuples(sortValue{assigned: a != 0, number: int64(a)}, sortValue{assigned: b != 0, number: int64(b)}, ids[i], ids[j], req.SortDirection == lanev1.SortDirection_SORT_DIRECTION_DESCENDING) < 0
		})
	} else {
		sort.Strings(ids)
	}
	return ids, nil
}

func decodeCompletedRequest(t *testing.T, data json.RawMessage) *lanev1.ListTasksRequest {
	t.Helper()
	req := new(lanev1.ListTasksRequest)
	if err := protojson.Unmarshal(data, req); err != nil {
		t.Fatal(err)
	}
	return req
}

func TestCompletedListVectors(t *testing.T) {
	data, err := os.ReadFile("fixtures/completed-list.json")
	if err != nil {
		t.Fatal(err)
	}
	var suite completedSuite
	if err := json.Unmarshal(data, &suite); err != nil {
		t.Fatal(err)
	}
	if suite.FormatVersion != 1 || len(suite.EntryCases) < 12 || len(suite.ListCases) < 8 {
		t.Fatalf("incomplete completed-list fixture: version=%d entries=%d lists=%d", suite.FormatVersion, len(suite.EntryCases), len(suite.ListCases))
	}
	queryInstant, err := time.Parse(time.RFC3339Nano, suite.QueryInstant)
	if err != nil {
		t.Fatal(err)
	}
	m := &completedReference{versions: map[string]*lanev1.EntityVersion{}, rows: suite.Dataset.Rows}
	for _, raw := range suite.Dataset.Versions {
		v := new(lanev1.EntityVersion)
		if err := protojson.Unmarshal(raw, v); err != nil {
			t.Fatal(err)
		}
		if v.VersionId == "" || m.versions[v.VersionId] != nil {
			t.Fatalf("missing or duplicate version id %s", v.VersionId)
		}
		m.versions[v.VersionId] = v
	}
	for _, c := range suite.EntryCases {
		t.Run("entry/"+c.ID, func(t *testing.T) {
			if len(c.Input.DisplayedHistoryVersionIDs) > 0 {
				selected := m.version(c.Input.SelectedVersionID)
				outsidePage := slices.ContainsFunc(selected.ParentVersionIds, func(id string) bool {
					return !slices.Contains(c.Input.DisplayedHistoryVersionIDs, id)
				})
				if !outsidePage {
					t.Fatal("history-page case needs an authoritative parent outside the page")
				}
			}
			want := new(lanev1.TerminalEntry)
			if err := protojson.Unmarshal(c.Expected, want); err != nil {
				t.Fatal(err)
			}
			if got := m.terminalEntry(c.Input); !proto.Equal(got, want) {
				t.Fatalf("entry got %v, want %v", got, want)
			}
		})
	}
	for _, c := range suite.ListCases {
		t.Run("list/"+c.ID, func(t *testing.T) {
			req := decodeCompletedRequest(t, c.Input.Request)
			if req.Completed != nil {
				var duration time.Duration
				switch req.Completed.Selection {
				case lanev1.CompletedSelection_COMPLETED_SELECTION_LAST_24_HOURS:
					duration = 24 * time.Hour
				case lanev1.CompletedSelection_COMPLETED_SELECTION_LAST_7_DAYS:
					duration = 7 * 24 * time.Hour
				case lanev1.CompletedSelection_COMPLETED_SELECTION_LAST_30_DAYS:
					duration = 30 * 24 * time.Hour
				}
				if duration > 0 && !proto.Equal(req.Completed.Cutoff, timestamppb.New(queryInstant.Add(-duration))) {
					t.Fatalf("cutoff does not match the fixed query instant and duration")
				}
			}
			got, err := m.list(req)
			if err != nil {
				t.Fatal(err)
			}
			if !slices.Equal(got, c.Expected.TaskIDs) {
				t.Fatalf("order got %v, want %v", got, c.Expected.TaskIDs)
			}
			size := int(req.GetPage().GetSize())
			if size <= 0 {
				t.Fatal("fixture page size must be positive")
			}
			var pages [][]string
			for start := 0; start < len(got); start += size {
				end := min(start+size, len(got))
				pages = append(pages, got[start:end])
			}
			if len(pages) == 0 {
				pages = [][]string{{}}
			}
			if !slices.EqualFunc(pages, c.Expected.Pages, slices.Equal) {
				t.Fatalf("pages got %v, want %v", pages, c.Expected.Pages)
			}
		})
	}
	for _, c := range suite.InvalidCases {
		t.Run("invalid/"+c.ID, func(t *testing.T) {
			if _, err := m.list(decodeCompletedRequest(t, c.Request)); err == nil {
				t.Fatal("invalid completed filter accepted")
			}
		})
	}
	for _, c := range suite.CursorCases {
		t.Run("cursor/"+c.ID, func(t *testing.T) {
			first := decodeCompletedRequest(t, c.First)
			resume := decodeCompletedRequest(t, c.Resume)
			first.Page, resume.Page = nil, nil
			valid := c.FirstAccountID == c.ResumeAccountID && c.FirstSpaceID == c.ResumeSpaceID && proto.Equal(first, resume)
			if valid != c.Valid {
				t.Fatalf("cursor scope valid=%t, want %t", valid, c.Valid)
			}
		})
	}
}
