// SPDX-License-Identifier: Apache-2.0
package lanev1_test

// Executable specification for the Lane planning rules. The reference model below applies the
// domain rules of the task-metadata and planning contract to fixture states so that the Rust
// store, the Rust server and the native C++ store can mirror it and replay the same fixtures.
// It is deliberately small, allocation-happy and free of storage concerns.

import (
	"encoding/json"
	"fmt"
	"os"
	"regexp"
	"slices"
	"sort"
	"strings"
	"testing"
	"unicode/utf8"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
)

const (
	maxNameScalars        = 100
	maxDescriptionScalars = 4000
	maxLabelsPerTask      = 100
	maxStoryPoints        = 2147483647
)

var colorPattern = regexp.MustCompile(`^#[0-9A-Fa-f]{6}$`)
var datePattern = regexp.MustCompile(`^[0-9]{4}-[0-9]{2}-[0-9]{2}$`)

// ruleError carries the ErrorDetail fields a rejected operation reports.
type ruleError struct {
	reason    lanev1.ErrorReason
	violation lanev1.ConstraintViolation
}

func (e *ruleError) Error() string { return e.reason.String() + "/" + e.violation.String() }

func invalid() *ruleError { return &ruleError{reason: lanev1.ErrorReason_ERROR_REASON_INVALID_INPUT} }
func violation(v lanev1.ConstraintViolation) *ruleError {
	return &ruleError{reason: lanev1.ErrorReason_ERROR_REASON_CONSTRAINT_VIOLATION, violation: v}
}

// validDate reports whether s is a real proleptic Gregorian YYYY-MM-DD date.
func validDate(s string) bool {
	if !datePattern.MatchString(s) {
		return false
	}
	var y, m, d int
	if _, err := fmt.Sscanf(s, "%d-%d-%d", &y, &m, &d); err != nil || m < 1 || m > 12 || d < 1 {
		return false
	}
	days := []int{31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31}
	if y%4 == 0 && (y%100 != 0 || y%400 == 0) {
		days[1] = 29
	}
	return d <= days[m-1]
}

// refState holds the live (non-deleted) head of every entity.
type refState struct {
	entities map[string]*lanev1.EntityVersion
}

func loadState(t *testing.T, raw json.RawMessage) *refState {
	t.Helper()
	var doc struct {
		Entities []json.RawMessage `json:"entities"`
	}
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatal(err)
	}
	st := &refState{entities: map[string]*lanev1.EntityVersion{}}
	for _, item := range doc.Entities {
		version := new(lanev1.EntityVersion)
		if err := protojson.Unmarshal(item, version); err != nil {
			t.Fatal(err)
		}
		if version.FormatVersion != 3 || version.EntityId == "" || version.Kind == lanev1.EntityKind_ENTITY_KIND_UNSPECIFIED {
			t.Fatalf("invalid state entity %s", version.EntityId)
		}
		if !version.Deleted {
			st.entities[version.EntityId] = version
		}
	}
	return st
}

func (s *refState) tasks() map[string]*lanev1.Task {
	out := map[string]*lanev1.Task{}
	for id, e := range s.entities {
		if t := e.GetTask(); t != nil {
			out[id] = proto.Clone(t).(*lanev1.Task)
		}
	}
	return out
}

func planningFields(body *lanev1.PlanningBody) (kind lanev1.EntityKind, repository, name, description string) {
	switch b := body.GetBody().(type) {
	case *lanev1.PlanningBody_Label:
		return lanev1.EntityKind_ENTITY_KIND_LABEL, b.Label.RepositoryId, b.Label.Name, b.Label.Description
	case *lanev1.PlanningBody_Milestone:
		return lanev1.EntityKind_ENTITY_KIND_MILESTONE, b.Milestone.RepositoryId, b.Milestone.Name, b.Milestone.Description
	case *lanev1.PlanningBody_Cycle:
		return lanev1.EntityKind_ENTITY_KIND_CYCLE, b.Cycle.RepositoryId, b.Cycle.Name, b.Cycle.Description
	}
	return lanev1.EntityKind_ENTITY_KIND_UNSPECIFIED, "", "", ""
}

func entityName(e *lanev1.EntityVersion) string {
	switch b := e.GetBody().(type) {
	case *lanev1.EntityVersion_Label:
		return b.Label.Name
	case *lanev1.EntityVersion_Milestone:
		return b.Milestone.Name
	case *lanev1.EntityVersion_Cycle:
		return b.Cycle.Name
	}
	return ""
}

func entityRepository(e *lanev1.EntityVersion) string {
	switch b := e.GetBody().(type) {
	case *lanev1.EntityVersion_Label:
		return b.Label.RepositoryId
	case *lanev1.EntityVersion_Milestone:
		return b.Milestone.RepositoryId
	case *lanev1.EntityVersion_Cycle:
		return b.Cycle.RepositoryId
	case *lanev1.EntityVersion_Task:
		return b.Task.RepositoryId
	}
	return ""
}

// writePlanning validates a create (existingID empty or unknown) or save of a planning item and
// returns the normalized body that would be stored.
func (s *refState) writePlanning(id string, body *lanev1.PlanningBody) (*lanev1.PlanningBody, *ruleError) {
	kind, repository, name, description := planningFields(body)
	if kind == lanev1.EntityKind_ENTITY_KIND_UNSPECIFIED || repository == "" {
		return nil, invalid()
	}
	if existing := s.entities[id]; existing != nil {
		// Kind and repository are immutable after creation.
		if existing.Kind != kind || entityRepository(existing) != repository {
			return nil, invalid()
		}
	}
	out := proto.Clone(body).(*lanev1.PlanningBody)
	name = strings.TrimSpace(name)
	if n := utf8.RuneCountInString(name); n < 1 || n > maxNameScalars {
		return nil, invalid()
	}
	if utf8.RuneCountInString(description) > maxDescriptionScalars {
		return nil, invalid()
	}
	switch b := out.GetBody().(type) {
	case *lanev1.PlanningBody_Label:
		b.Label.Name = name
		if !colorPattern.MatchString(b.Label.Color) {
			return nil, invalid()
		}
		b.Label.Color = strings.ToUpper(b.Label.Color)
	case *lanev1.PlanningBody_Milestone:
		b.Milestone.Name = name
		if b.Milestone.TargetDate != nil && !validDate(*b.Milestone.TargetDate) {
			return nil, invalid()
		}
	case *lanev1.PlanningBody_Cycle:
		b.Cycle.Name = name
		if !validDate(b.Cycle.StartDate) || !validDate(b.Cycle.EndDate) {
			return nil, invalid()
		}
		if b.Cycle.StartDate > b.Cycle.EndDate {
			return nil, violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_DATE_ORDER)
		}
	}
	for otherID, other := range s.entities {
		if otherID != id && other.Kind == kind && entityRepository(other) == repository && strings.ToLower(entityName(other)) == strings.ToLower(name) {
			return nil, violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_DUPLICATE_NAME)
		}
	}
	return out, nil
}

func sortedUnique(in []string) []string {
	out := slices.Clone(in)
	sort.Strings(out)
	out = slices.Compact(out)
	if len(out) == 0 {
		return nil
	}
	return out
}

// checkPlanningRef verifies that id is a live item of the wanted kind in the task's repository.
func (s *refState) checkPlanningRef(id string, kind lanev1.EntityKind, repository string) *ruleError {
	e := s.entities[id]
	if e == nil || e.Kind != kind {
		return violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND)
	}
	if entityRepository(e) != repository {
		return violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_CROSS_REPOSITORY)
	}
	return nil
}

// validateTaskFields checks task-local rules and normalizes the stored sets.
func (s *refState) validateTaskFields(t *lanev1.Task) *ruleError {
	if _, ok := lanev1.TaskPriority_name[int32(t.Priority)]; !ok {
		return invalid()
	}
	if _, ok := lanev1.TaskType_name[int32(t.Type)]; !ok {
		return invalid()
	}
	for _, d := range []*string{t.StartDate, t.DueDate} {
		if d != nil && !validDate(*d) {
			return invalid()
		}
	}
	if t.StoryPoints != nil && *t.StoryPoints > maxStoryPoints {
		return violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_FIELD_RANGE)
	}
	if t.StartDate != nil && t.DueDate != nil && *t.StartDate > *t.DueDate {
		return violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_DATE_ORDER)
	}
	t.LabelIds = sortedUnique(t.LabelIds)
	if len(t.LabelIds) > maxLabelsPerTask {
		return violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_LABEL_LIMIT)
	}
	for _, id := range t.LabelIds {
		if err := s.checkPlanningRef(id, lanev1.EntityKind_ENTITY_KIND_LABEL, t.RepositoryId); err != nil {
			return err
		}
	}
	if t.MilestoneId != nil {
		if err := s.checkPlanningRef(*t.MilestoneId, lanev1.EntityKind_ENTITY_KIND_MILESTONE, t.RepositoryId); err != nil {
			return err
		}
	}
	if t.CycleId != nil {
		if err := s.checkPlanningRef(*t.CycleId, lanev1.EntityKind_ENTITY_KIND_CYCLE, t.RepositoryId); err != nil {
			return err
		}
	}
	t.DependsOnTaskIds = sortedUnique(t.DependsOnTaskIds)
	t.RelatedTaskIds = sortedUnique(t.RelatedTaskIds)
	return nil
}

func clearReferences(t *lanev1.Task) {
	t.LabelIds, t.MilestoneId, t.CycleId = nil, nil, nil
	t.ParentTaskId, t.DependsOnTaskIds, t.RelatedTaskIds, t.DuplicateOfTaskId = nil, nil, nil, nil
}

// removeTaskReference drops every reference to id from t and reports whether t changed.
func removeTaskReference(t *lanev1.Task, id string) bool {
	changed := false
	if t.ParentTaskId != nil && *t.ParentTaskId == id {
		t.ParentTaskId, changed = nil, true
	}
	if t.DuplicateOfTaskId != nil && *t.DuplicateOfTaskId == id {
		t.DuplicateOfTaskId, changed = nil, true
	}
	if i := slices.Index(t.DependsOnTaskIds, id); i >= 0 {
		t.DependsOnTaskIds, changed = slices.Delete(t.DependsOnTaskIds, i, i+1), true
		if len(t.DependsOnTaskIds) == 0 {
			t.DependsOnTaskIds = nil
		}
	}
	if i := slices.Index(t.RelatedTaskIds, id); i >= 0 {
		t.RelatedTaskIds, changed = slices.Delete(t.RelatedTaskIds, i, i+1), true
		if len(t.RelatedTaskIds) == 0 {
			t.RelatedTaskIds = nil
		}
	}
	return changed
}

// writeResult lists every task body the operation stores plus the ids it tombstones.
type writeResult struct {
	primary *lanev1.Task            // new body of the written task; nil for deletes
	changed map[string]*lanev1.Task // every other task whose stored body changes
	deleted []string
}

func diffTasks(before, after map[string]*lanev1.Task, skip string) map[string]*lanev1.Task {
	changed := map[string]*lanev1.Task{}
	for id, t := range after {
		if id != skip && !proto.Equal(before[id], t) {
			changed[id] = t
		}
	}
	return changed
}

// writeTask validates and applies a create (create=true) or save of a task. Moving a task to
// another repository discards the reference fields sent with the move and clears the stored ones.
func (s *refState) writeTask(id string, in *lanev1.Task, edits *lanev1.RelationshipEdits, create bool) (*writeResult, *ruleError) {
	before := s.tasks()
	work := s.tasks()
	t := proto.Clone(in).(*lanev1.Task)
	if prev := before[id]; !create && prev != nil && prev.RepositoryId != t.RepositoryId {
		clearReferences(t)
		edits = nil
		if err := s.validateTaskFields(t); err != nil {
			return nil, err
		}
		work[id] = t
		for otherID, other := range work {
			if otherID != id {
				removeTaskReference(other, id)
			}
		}
		return &writeResult{primary: t, changed: diffTasks(before, work, id)}, nil
	}
	if err := s.validateTaskFields(t); err != nil {
		return nil, err
	}
	// Collect every task id this write names so the shared reference checks run once.
	type link struct {
		id    string
		edits bool
	}
	var links []link
	for _, ref := range [][]string{t.DependsOnTaskIds, t.RelatedTaskIds} {
		for _, r := range ref {
			links = append(links, link{r, false})
		}
	}
	for _, r := range []*string{t.ParentTaskId, t.DuplicateOfTaskId} {
		if r != nil {
			links = append(links, link{*r, false})
		}
	}
	if edits != nil {
		for _, ref := range [][]string{edits.AddSubtaskIds, edits.AddBlockedTaskIds} {
			for _, r := range ref {
				links = append(links, link{r, true})
			}
		}
	}
	for _, l := range links {
		if l.id == id {
			return nil, violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_SELF_LINK)
		}
	}
	for _, l := range links {
		if work[l.id] == nil {
			return nil, violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND)
		}
	}
	for _, l := range links {
		if work[l.id].RepositoryId != t.RepositoryId {
			return nil, violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_CROSS_REPOSITORY)
		}
	}
	// Related links are stored on both endpoints.
	prevRelated := []string(nil)
	if prev := before[id]; prev != nil {
		prevRelated = prev.RelatedTaskIds
	}
	for _, r := range slices.Concat(prevRelated, t.RelatedTaskIds) {
		other := work[r]
		if other == nil {
			continue
		}
		other.RelatedTaskIds = sortedUnique(slices.DeleteFunc(slices.Clone(other.RelatedTaskIds), func(x string) bool { return x == id }))
		if slices.Contains(t.RelatedTaskIds, r) {
			other.RelatedTaskIds = sortedUnique(append(other.RelatedTaskIds, id))
		}
	}
	work[id] = t
	if edits != nil {
		for _, r := range sortedUnique(edits.AddSubtaskIds) {
			work[r].ParentTaskId = proto.String(id)
		}
		for _, r := range sortedUnique(edits.RemoveSubtaskIds) {
			if other := work[r]; other != nil && other.ParentTaskId != nil && *other.ParentTaskId == id {
				other.ParentTaskId = nil
			}
		}
		for _, r := range sortedUnique(edits.AddBlockedTaskIds) {
			work[r].DependsOnTaskIds = sortedUnique(append(work[r].DependsOnTaskIds, id))
		}
		for _, r := range sortedUnique(edits.RemoveBlockedTaskIds) {
			if other := work[r]; other != nil {
				other.DependsOnTaskIds = sortedUnique(slices.DeleteFunc(slices.Clone(other.DependsOnTaskIds), func(x string) bool { return x == id }))
			}
		}
	}
	if hasPointerCycle(work, func(x *lanev1.Task) *string { return x.ParentTaskId }) {
		return nil, violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_PARENT_CYCLE)
	}
	if hasDependencyCycle(work) {
		return nil, violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_DEPENDENCY_CYCLE)
	}
	if hasPointerCycle(work, func(x *lanev1.Task) *string { return x.DuplicateOfTaskId }) {
		return nil, violation(lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_DUPLICATE_CYCLE)
	}
	return &writeResult{primary: t, changed: diffTasks(before, work, id)}, nil
}

// hasPointerCycle follows a single-valued link (parent or duplicate-of) from every task.
func hasPointerCycle(tasks map[string]*lanev1.Task, next func(*lanev1.Task) *string) bool {
	for start := range tasks {
		seen := map[string]bool{start: true}
		for cur := next(tasks[start]); cur != nil; {
			if seen[*cur] {
				return true
			}
			seen[*cur] = true
			target := tasks[*cur]
			if target == nil {
				break
			}
			cur = next(target)
		}
	}
	return false
}

func hasDependencyCycle(tasks map[string]*lanev1.Task) bool {
	const (
		visiting = 1
		done     = 2
	)
	state := map[string]int{}
	var visit func(string) bool
	visit = func(id string) bool {
		switch state[id] {
		case visiting:
			return true
		case done:
			return false
		}
		state[id] = visiting
		if t := tasks[id]; t != nil {
			for _, dep := range t.DependsOnTaskIds {
				if visit(dep) {
					return true
				}
			}
		}
		state[id] = done
		return false
	}
	for id := range tasks {
		if visit(id) {
			return true
		}
	}
	return false
}

// deleteTask tombstones id and removes every incoming reference from other tasks.
func (s *refState) deleteTask(id string) *writeResult {
	before := s.tasks()
	work := s.tasks()
	delete(work, id)
	for _, other := range work {
		removeTaskReference(other, id)
	}
	return &writeResult{changed: diffTasks(before, work, id), deleted: []string{id}}
}

// deletePlanning tombstones a label, milestone or cycle and removes the assignment from tasks.
func (s *refState) deletePlanning(id string) *writeResult {
	before := s.tasks()
	work := s.tasks()
	for _, t := range work {
		t.LabelIds = slices.DeleteFunc(t.LabelIds, func(x string) bool { return x == id })
		if len(t.LabelIds) == 0 {
			t.LabelIds = nil
		}
		if t.MilestoneId != nil && *t.MilestoneId == id {
			t.MilestoneId = nil
		}
		if t.CycleId != nil && *t.CycleId == id {
			t.CycleId = nil
		}
	}
	return &writeResult{changed: diffTasks(before, work, ""), deleted: []string{id}}
}

// taskFromCreate maps the flat CreateTask fields to the stored task body.
func taskFromCreate(r *lanev1.CreateTaskRequest) *lanev1.Task {
	t := &lanev1.Task{
		RepositoryId: r.GetRepository().GetId(), Title: r.Title, Description: r.Description, Prompt: r.Prompt,
		Progress: r.Progress, Proposal: r.Proposal, Priority: r.Priority, Type: r.Type, LabelIds: r.LabelIds,
		StartDate: r.StartDate, DueDate: r.DueDate, StoryPoints: r.StoryPoints, MilestoneId: r.MilestoneId, CycleId: r.CycleId,
		ParentTaskId: r.ParentTaskId, DependsOnTaskIds: r.DependsOnTaskIds, RelatedTaskIds: r.RelatedTaskIds, DuplicateOfTaskId: r.DuplicateOfTaskId,
	}
	if t.Progress == lanev1.Progress_PROGRESS_UNSPECIFIED {
		t.Progress = lanev1.Progress_PROGRESS_TODO
	}
	if t.Proposal == lanev1.Proposal_PROPOSAL_UNSPECIFIED {
		t.Proposal = lanev1.Proposal_PROPOSAL_REGULAR
	}
	return t
}

type planningSuite struct {
	State json.RawMessage `json:"state"`
	Cases []struct {
		ID    string `json:"id"`
		Input struct {
			Method  string          `json:"method"`
			Request json.RawMessage `json:"request"`
		} `json:"input"`
		Expected json.RawMessage `json:"expected"`
	} `json:"cases"`
}

func loadSuite(t *testing.T, path string) *planningSuite {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	suite := new(planningSuite)
	if err := json.Unmarshal(data, suite); err != nil {
		t.Fatal(err)
	}
	return suite
}

type outcomeExpectation struct {
	Outcome    string          `json:"outcome"`
	Reason     string          `json:"reason"`
	Violation  string          `json:"violation"`
	Normalized json.RawMessage `json:"normalized"`
}

func assertRuleError(t *testing.T, got *ruleError, want outcomeExpectation) {
	t.Helper()
	if got == nil {
		t.Fatalf("operation succeeded, want %s %s", want.Reason, want.Violation)
	}
	wantViolation := want.Violation
	if wantViolation == "" {
		wantViolation = lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_UNSPECIFIED.String()
	}
	if got.reason.String() != want.Reason || got.violation.String() != wantViolation {
		t.Fatalf("got %s/%s, want %s/%s", got.reason, got.violation, want.Reason, wantViolation)
	}
}

func decodeTaskBody(t *testing.T, raw []byte) *lanev1.Task {
	t.Helper()
	task := new(lanev1.Task)
	if err := protojson.Unmarshal(raw, task); err != nil {
		t.Fatal(err)
	}
	return task
}

func assertAffected(t *testing.T, got map[string]*lanev1.Task, want []json.RawMessage) {
	t.Helper()
	wantByID := map[string]*lanev1.Task{}
	var order []string
	for _, raw := range want {
		version := new(lanev1.EntityVersion)
		if err := protojson.Unmarshal(raw, version); err != nil {
			t.Fatal(err)
		}
		wantByID[version.EntityId] = version.GetTask()
		order = append(order, version.EntityId)
	}
	if !slices.IsSorted(order) {
		t.Fatalf("fixture lists changed entities out of id order: %v", order)
	}
	if len(got) != len(wantByID) {
		t.Fatalf("changed %d tasks, fixture lists %d", len(got), len(wantByID))
	}
	for id, task := range got {
		if w := wantByID[id]; w == nil || !proto.Equal(w, task) {
			t.Fatalf("changed task %s = %v, want %v", id, task, w)
		}
	}
}

func TestPlanningValidationVectors(t *testing.T) {
	suite := loadSuite(t, "fixtures/planning-validation.json")
	for _, c := range suite.Cases {
		t.Run(c.ID, func(t *testing.T) {
			st := loadState(t, suite.State)
			var want outcomeExpectation
			if err := json.Unmarshal(c.Expected, &want); err != nil {
				t.Fatal(err)
			}
			var gotBody proto.Message
			var rerr *ruleError
			switch c.Input.Method {
			case "CreatePlanningItem":
				req := new(lanev1.CreatePlanningItemRequest)
				mustUnmarshal(t, c.Input.Request, req)
				gotBody, rerr = nilSafe(st.writePlanning(req.Id, req.Item))
			case "SavePlanningItem":
				req := new(lanev1.SavePlanningItemRequest)
				mustUnmarshal(t, c.Input.Request, req)
				gotBody, rerr = nilSafe(st.writePlanning(req.Id, req.Item))
			case "SaveTask":
				req := new(lanev1.SaveTaskRequest)
				mustUnmarshal(t, c.Input.Request, req)
				var res *writeResult
				res, rerr = st.writeTask(req.Id, req.Task, req.RelationshipEdits, false)
				if rerr == nil {
					gotBody = res.primary
				}
			default:
				t.Fatalf("unknown method %s", c.Input.Method)
			}
			checkOutcome(t, want, gotBody, rerr)
		})
	}
}

func nilSafe(body *lanev1.PlanningBody, err *ruleError) (proto.Message, *ruleError) {
	if err != nil {
		return nil, err
	}
	return body, nil
}

func mustUnmarshal(t *testing.T, raw []byte, m proto.Message) {
	t.Helper()
	if err := protojson.Unmarshal(raw, m); err != nil {
		t.Fatal(err)
	}
}

func checkOutcome(t *testing.T, want outcomeExpectation, got proto.Message, rerr *ruleError) {
	t.Helper()
	switch want.Outcome {
	case "error":
		assertRuleError(t, rerr, want)
	case "ok":
		if rerr != nil {
			t.Fatalf("unexpected rejection %v", rerr)
		}
		var wantMessage proto.Message
		var normalized map[string]json.RawMessage
		if err := json.Unmarshal(want.Normalized, &normalized); err != nil {
			t.Fatal(err)
		}
		if _, isTask := normalized["task"]; isTask {
			wantMessage = decodeTaskBody(t, normalized["task"])
		} else {
			wantMessage = new(lanev1.PlanningBody)
			mustUnmarshal(t, want.Normalized, wantMessage)
		}
		if !proto.Equal(wantMessage, got) {
			t.Fatalf("normalized body = %v, want %v", got, wantMessage)
		}
	default:
		t.Fatalf("unknown outcome %q", want.Outcome)
	}
}

func TestPlanningGraphVectors(t *testing.T) {
	suite := loadSuite(t, "fixtures/planning-graph.json")
	for _, c := range suite.Cases {
		t.Run(c.ID, func(t *testing.T) {
			st := loadState(t, suite.State)
			var want struct {
				outcomeExpectation
				Affected []json.RawMessage `json:"affected"`
			}
			if err := json.Unmarshal(c.Expected, &want); err != nil {
				t.Fatal(err)
			}
			var res *writeResult
			var rerr *ruleError
			switch c.Input.Method {
			case "SaveTask":
				req := new(lanev1.SaveTaskRequest)
				mustUnmarshal(t, c.Input.Request, req)
				res, rerr = st.writeTask(req.Id, req.Task, req.RelationshipEdits, false)
			case "CreateTask":
				req := new(lanev1.CreateTaskRequest)
				mustUnmarshal(t, c.Input.Request, req)
				res, rerr = st.writeTask(req.Id, taskFromCreate(req), req.RelationshipEdits, true)
			default:
				t.Fatalf("unknown method %s", c.Input.Method)
			}
			var primary proto.Message
			if res != nil {
				primary = res.primary
			}
			checkOutcome(t, want.outcomeExpectation, primary, rerr)
			if want.Outcome == "ok" {
				assertAffected(t, res.changed, want.Affected)
			}
		})
	}
}

func TestPlanningCleanupVectors(t *testing.T) {
	suite := loadSuite(t, "fixtures/planning-cleanup.json")
	for _, c := range suite.Cases {
		t.Run(c.ID, func(t *testing.T) {
			st := loadState(t, suite.State)
			var want struct {
				Deleted []string          `json:"deleted"`
				Changed []json.RawMessage `json:"changed"`
			}
			if err := json.Unmarshal(c.Expected, &want); err != nil {
				t.Fatal(err)
			}
			var res *writeResult
			switch c.Input.Method {
			case "SaveTask":
				req := new(lanev1.SaveTaskRequest)
				mustUnmarshal(t, c.Input.Request, req)
				var rerr *ruleError
				res, rerr = st.writeTask(req.Id, req.Task, req.RelationshipEdits, false)
				if rerr != nil {
					t.Fatal(rerr)
				}
				res.changed[req.Id] = res.primary
			case "DeleteTask":
				req := new(lanev1.DeleteTaskRequest)
				mustUnmarshal(t, c.Input.Request, req)
				res = st.deleteTask(req.Id)
			case "DeletePlanningItem":
				req := new(lanev1.DeletePlanningItemRequest)
				mustUnmarshal(t, c.Input.Request, req)
				res = st.deletePlanning(req.Id)
			default:
				t.Fatalf("unknown method %s", c.Input.Method)
			}
			if !slices.Equal(res.deleted, want.Deleted) {
				t.Fatalf("deleted %v, want %v", res.deleted, want.Deleted)
			}
			assertAffected(t, res.changed, want.Changed)
		})
	}
}

func TestPlanningFixtureCoverage(t *testing.T) {
	// Every violation code and the generic rejection must be exercised by at least one vector.
	seen := map[string]bool{}
	for _, path := range []string{"fixtures/planning-validation.json", "fixtures/planning-graph.json"} {
		for _, c := range loadSuite(t, path).Cases {
			var want outcomeExpectation
			if err := json.Unmarshal(c.Expected, &want); err != nil {
				t.Fatal(err)
			}
			seen[want.Violation] = true
			seen[want.Reason] = true
		}
	}
	for number, name := range lanev1.ConstraintViolation_name {
		if number != 0 && !seen[name] {
			t.Errorf("no fixture exercises %s", name)
		}
	}
	if !seen[lanev1.ErrorReason_ERROR_REASON_INVALID_INPUT.String()] {
		t.Error("no INVALID_INPUT vector")
	}
}
