// SPDX-License-Identifier: Apache-2.0
package lanev1_test

// Reference evaluator for ListTasks filtering, sorting and keyset pagination. It is the
// executable specification the Rust store, Rust server and native C++ store mirror.

import (
	"encoding/json"
	"os"
	"slices"
	"sort"
	"strings"
	"testing"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/encoding/protojson"
)

// listModel is the full live task set plus the planning items sort keys resolve through.
type listModel struct {
	tasks map[string]*lanev1.Task
	state *refState
}

func newListModel(st *refState) *listModel {
	return &listModel{tasks: st.tasks(), state: st}
}

// sortValue is one task's key. Unassigned values have assigned=false and always sort last.
type sortValue struct {
	assigned bool
	number   int64
	text     string
}

func (m *listModel) sortValue(t *lanev1.Task, key lanev1.TaskSortKey) sortValue {
	text := func(s *string) sortValue {
		if s == nil {
			return sortValue{}
		}
		return sortValue{assigned: true, text: *s}
	}
	switch key {
	case lanev1.TaskSortKey_TASK_SORT_KEY_PRIORITY:
		// Urgent=1 .. Low=4, so ascending is Urgent first; UNSPECIFIED (0) is unassigned.
		return sortValue{assigned: t.Priority != lanev1.TaskPriority_TASK_PRIORITY_UNSPECIFIED, number: int64(t.Priority)}
	case lanev1.TaskSortKey_TASK_SORT_KEY_TYPE:
		return sortValue{assigned: t.Type != lanev1.TaskType_TASK_TYPE_UNSPECIFIED, number: int64(t.Type)}
	case lanev1.TaskSortKey_TASK_SORT_KEY_START_DATE:
		return text(t.StartDate)
	case lanev1.TaskSortKey_TASK_SORT_KEY_DUE_DATE:
		return text(t.DueDate)
	case lanev1.TaskSortKey_TASK_SORT_KEY_STORY_POINTS:
		if t.StoryPoints == nil {
			return sortValue{}
		}
		return sortValue{assigned: true, number: int64(*t.StoryPoints)}
	case lanev1.TaskSortKey_TASK_SORT_KEY_MILESTONE_NAME:
		if t.MilestoneId == nil {
			return sortValue{}
		}
		// Simple Unicode lower-casing, then code point order (Go compares UTF-8 bytes, which
		// is code point order).
		return sortValue{assigned: true, text: strings.ToLower(entityName(m.state.entities[*t.MilestoneId]))}
	case lanev1.TaskSortKey_TASK_SORT_KEY_CYCLE_START_DATE:
		if t.CycleId == nil {
			return sortValue{}
		}
		return sortValue{assigned: true, text: m.state.entities[*t.CycleId].GetCycle().StartDate}
	}
	return sortValue{}
}

// compare orders (value, id) tuples under the request direction: unassigned last in both
// directions, ties by ascending task id.
func compareTuples(a, b sortValue, aID, bID string, descending bool) int {
	if a.assigned != b.assigned {
		if a.assigned {
			return -1
		}
		return 1
	}
	if a.assigned {
		c := 0
		switch {
		case a.number != b.number:
			c = 1
			if a.number < b.number {
				c = -1
			}
		case a.text != b.text:
			c = strings.Compare(a.text, b.text)
		}
		if descending {
			c = -c
		}
		if c != 0 {
			return c
		}
	}
	return strings.Compare(aID, bID)
}

// relationshipKinds derives the relationship views of a task from the whole task set.
func (m *listModel) relationshipKinds(id string) map[lanev1.RelationshipKind]bool {
	t := m.tasks[id]
	kinds := map[lanev1.RelationshipKind]bool{}
	if t.ParentTaskId != nil {
		kinds[lanev1.RelationshipKind_RELATIONSHIP_KIND_PARENT] = true
	}
	if len(t.DependsOnTaskIds) > 0 {
		kinds[lanev1.RelationshipKind_RELATIONSHIP_KIND_DEPENDS_ON] = true
	}
	if len(t.RelatedTaskIds) > 0 {
		kinds[lanev1.RelationshipKind_RELATIONSHIP_KIND_RELATED] = true
	}
	if t.DuplicateOfTaskId != nil {
		kinds[lanev1.RelationshipKind_RELATIONSHIP_KIND_DUPLICATE_OF] = true
	}
	for otherID, o := range m.tasks {
		if o.ParentTaskId != nil && *o.ParentTaskId == id {
			kinds[lanev1.RelationshipKind_RELATIONSHIP_KIND_SUBTASKS] = true
		}
		if slices.Contains(o.DependsOnTaskIds, id) {
			kinds[lanev1.RelationshipKind_RELATIONSHIP_KIND_BLOCKS] = true
		}
		if slices.Contains(o.RelatedTaskIds, id) && otherID != id {
			kinds[lanev1.RelationshipKind_RELATIONSHIP_KIND_RELATED] = true
		}
	}
	return kinds
}

// selected implements "selection OR unassigned flag"; no selection and a false flag never restricts.
func selected[T comparable](selection []T, includeUnassigned bool, value T, assigned bool) bool {
	if len(selection) == 0 && !includeUnassigned {
		return true
	}
	if !assigned {
		return includeUnassigned
	}
	return slices.Contains(selection, value)
}

func inDateRange(r *lanev1.DateRange, value *string) bool {
	if r == nil || (r.From == nil && r.To == nil && !r.IncludeUnassigned) {
		return true
	}
	if value == nil {
		return r.IncludeUnassigned
	}
	if r.From == nil && r.To == nil {
		return false // the flag alone selects only unassigned values
	}
	return (r.From == nil || *value >= *r.From) && (r.To == nil || *value <= *r.To)
}

func inPointRange(r *lanev1.PointRange, value *uint32) bool {
	if r == nil || (r.Min == nil && r.Max == nil && !r.IncludeUnassigned) {
		return true
	}
	if value == nil {
		return r.IncludeUnassigned
	}
	if r.Min == nil && r.Max == nil {
		return false
	}
	return (r.Min == nil || *value >= *r.Min) && (r.Max == nil || *value <= *r.Max)
}

func (m *listModel) matches(id string, req *lanev1.ListTasksRequest) bool {
	t := m.tasks[id]
	if req.RepositoryId != nil && t.RepositoryId != *req.RepositoryId {
		return false
	}
	if len(req.Progress) > 0 && !slices.Contains(req.Progress, t.Progress) {
		return false
	}
	if len(req.Proposal) > 0 && !slices.Contains(req.Proposal, t.Proposal) {
		return false
	}
	f := req.Filter
	if f == nil {
		return true
	}
	if !selected(f.Priorities, f.IncludeUnassignedPriority, t.Priority, t.Priority != lanev1.TaskPriority_TASK_PRIORITY_UNSPECIFIED) {
		return false
	}
	if !selected(f.Types, f.IncludeUnassignedType, t.Type, t.Type != lanev1.TaskType_TASK_TYPE_UNSPECIFIED) {
		return false
	}
	if len(f.LabelIds) > 0 || f.IncludeUnlabeled {
		any := slices.ContainsFunc(t.LabelIds, func(l string) bool { return slices.Contains(f.LabelIds, l) })
		if !any && !(f.IncludeUnlabeled && len(t.LabelIds) == 0) {
			return false
		}
	}
	if !inDateRange(f.StartDate, t.StartDate) || !inDateRange(f.DueDate, t.DueDate) || !inPointRange(f.StoryPoints, t.StoryPoints) {
		return false
	}
	milestone, cycle := "", ""
	if t.MilestoneId != nil {
		milestone = *t.MilestoneId
	}
	if t.CycleId != nil {
		cycle = *t.CycleId
	}
	if !selected(f.MilestoneIds, f.IncludeNoMilestone, milestone, t.MilestoneId != nil) || !selected(f.CycleIds, f.IncludeNoCycle, cycle, t.CycleId != nil) {
		return false
	}
	if len(f.Relationships) > 0 || f.IncludeNoRelationships {
		kinds := m.relationshipKinds(id)
		any := slices.ContainsFunc(f.Relationships, func(k lanev1.RelationshipKind) bool { return kinds[k] })
		if !any && !(f.IncludeNoRelationships && len(kinds) == 0) {
			return false
		}
	}
	return true
}

// order returns the unpaged result order.
func (m *listModel) order(req *lanev1.ListTasksRequest) []string {
	var ids []string
	for id := range m.tasks {
		if m.matches(id, req) {
			ids = append(ids, id)
		}
	}
	descending := req.SortDirection == lanev1.SortDirection_SORT_DIRECTION_DESCENDING
	sort.Slice(ids, func(i, j int) bool {
		return compareTuples(m.sortValue(m.tasks[ids[i]], req.SortKey), m.sortValue(m.tasks[ids[j]], req.SortKey), ids[i], ids[j], descending) < 0
	})
	return ids
}

// listCursor is the keyset position: the sort tuple and id of the last returned task.
type listCursor struct {
	value sortValue
	id    string
}

// page returns the tasks strictly after the cursor and the cursor for the next page, or nil.
func (m *listModel) page(req *lanev1.ListTasksRequest, after *listCursor, size int) ([]string, *listCursor) {
	descending := req.SortDirection == lanev1.SortDirection_SORT_DIRECTION_DESCENDING
	var rest []string
	for _, id := range m.order(req) {
		if after != nil && compareTuples(m.sortValue(m.tasks[id], req.SortKey), after.value, id, after.id, descending) <= 0 {
			continue
		}
		rest = append(rest, id)
	}
	if len(rest) <= size {
		return rest, nil
	}
	last := rest[size-1]
	return rest[:size], &listCursor{value: m.sortValue(m.tasks[last], req.SortKey), id: last}
}

func TestListVectors(t *testing.T) {
	data, err := os.ReadFile("fixtures/planning-list.json")
	if err != nil {
		t.Fatal(err)
	}
	var suite struct {
		Dataset json.RawMessage `json:"dataset"`
		Cases   []struct {
			ID    string `json:"id"`
			Input struct {
				Request json.RawMessage `json:"request"`
			} `json:"input"`
			Expected struct {
				TaskIDs []string   `json:"taskIds"`
				Pages   [][]string `json:"pages"`
			} `json:"expected"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(data, &suite); err != nil {
		t.Fatal(err)
	}
	model := newListModel(loadState(t, suite.Dataset))
	assertListDatasetShape(t, model)
	if len(suite.Cases) < 40 {
		t.Fatalf("only %d list vectors", len(suite.Cases))
	}
	for _, c := range suite.Cases {
		t.Run(c.ID, func(t *testing.T) {
			req := new(lanev1.ListTasksRequest)
			if err := protojson.Unmarshal(c.Input.Request, req); err != nil {
				t.Fatal(err)
			}
			if got := model.order(req); !slices.Equal(got, c.Expected.TaskIDs) {
				t.Fatalf("order\n got %v\nwant %v", got, c.Expected.TaskIDs)
			}
			// Keyset pagination must reproduce the unpaged order page by page.
			size := int(req.Page.Size)
			var pages [][]string
			var cursor *listCursor
			for {
				ids, next := model.page(req, cursor, size)
				pages = append(pages, append([]string{}, ids...))
				if next == nil {
					break
				}
				cursor = next
			}
			if len(pages) != len(c.Expected.Pages) {
				t.Fatalf("pages %v, want %v", pages, c.Expected.Pages)
			}
			for i := range pages {
				if !slices.Equal(pages[i], c.Expected.Pages[i]) {
					t.Fatalf("page %d = %v, want %v", i, pages[i], c.Expected.Pages[i])
				}
			}
			if flat := slices.Concat(pages...); !slices.Equal(slices.Concat(c.Expected.Pages...), flat) || (len(flat) > 0 && !slices.Equal(flat, c.Expected.TaskIDs)) {
				t.Fatal("concatenated pages differ from the unpaged order")
			}
		})
	}
}

// assertListDatasetShape pins the dataset properties the list vectors rely on.
func assertListDatasetShape(t *testing.T, m *listModel) {
	t.Helper()
	repositories := map[string]bool{}
	zeroPoints, absentPoints, unlabeled, priorityTie := false, false, false, map[lanev1.TaskPriority]int{}
	for _, task := range m.tasks {
		repositories[task.RepositoryId] = true
		zeroPoints = zeroPoints || (task.StoryPoints != nil && *task.StoryPoints == 0)
		absentPoints = absentPoints || task.StoryPoints == nil
		unlabeled = unlabeled || len(task.LabelIds) == 0
		priorityTie[task.Priority]++
	}
	if len(m.tasks) < 12 || len(repositories) != 2 || !zeroPoints || !absentPoints || !unlabeled {
		t.Fatalf("dataset lacks required shape: %d tasks, %d repositories", len(m.tasks), len(repositories))
	}
	if priorityTie[lanev1.TaskPriority_TASK_PRIORITY_HIGH] < 2 || priorityTie[lanev1.TaskPriority_TASK_PRIORITY_UNSPECIFIED] < 2 {
		t.Fatal("dataset needs sort ties")
	}
	// Labels with identical names must exist in both repositories under different ids.
	names := map[string][]string{}
	for id, e := range m.state.entities {
		if e.Kind == lanev1.EntityKind_ENTITY_KIND_LABEL {
			names[strings.ToLower(entityName(e))] = append(names[strings.ToLower(entityName(e))], id)
		}
	}
	shared := false
	for _, ids := range names {
		shared = shared || len(ids) == 2
	}
	if !shared {
		t.Fatal("dataset needs identically named labels in both repositories")
	}
}
