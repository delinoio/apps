// SPDX-License-Identifier: FSL-1.1-ALv2

package measurement

import (
	"errors"
	"fmt"
	"sort"
	"time"

	pb "github.com/delinoio/apps/protos/gittracker/v1"
	"google.golang.org/protobuf/proto"
)

type Baseline struct {
	Execution string `json:"execution"`
	Commit    string `json:"commit"`
}
type Record struct {
	RemovedDatasets map[string]bool     `json:"removedDatasets,omitempty"`
	Execution       *pb.Execution       `json:"execution"`
	LastData        time.Time           `json:"lastData"`
	Cutoff          time.Time           `json:"cutoff"`
	Ancestors       []string            `json:"ancestors"`
	Base            Baseline            `json:"base"`
	Push            bool                `json:"push"`
	Baselines       map[string]Baseline `json:"baselines"`
}
type State struct {
	DeletedExecutions map[string]bool    `json:"deletedExecutions,omitempty"`
	Next              uint64             `json:"next"`
	Records           map[string]*Record `json:"records"`
}

func NewState() *State { return &State{Records: map[string]*Record{}} }
func (s *State) Interrupt(now time.Time) int {
	count := 0
	for _, r := range s.Records {
		if r.Execution.State == pb.ExecutionState_EXECUTION_STATE_MEASURING && !now.Before(r.LastData.Add(6*time.Hour)) {
			r.Execution.State = pb.ExecutionState_EXECUTION_STATE_INTERRUPTED
			count++
		}
	}
	return count
}
func (s *State) Representative(commit string, cutoff time.Time) *Record {
	return s.representative(commit, cutoff, false)
}
func (s *State) BaselineRepresentative(commit string, cutoff time.Time) *Record {
	return s.representative(commit, cutoff, true)
}
func (s *State) representative(commit string, cutoff time.Time, branchOnly bool) *Record {
	var found *Record
	for _, r := range s.Records {
		e := r.Execution
		if branchOnly && e.Context.PullRequest != 0 {
			continue
		}
		if e.Context.Commit != commit || e.State != pb.ExecutionState_EXECUTION_STATE_COMPLETED {
			continue
		}
		t, _ := time.Parse(time.RFC3339Nano, e.CompletedAt)
		if !cutoff.IsZero() && t.After(cutoff) {
			continue
		}
		if found == nil || e.RegistrationOrder > found.Execution.RegistrationOrder {
			found = r
		}
	}
	return found
}
func (s *State) Register(c *pb.Context, now time.Time, ancestors []string, base string) *Record {
	s.Next++
	r := &Record{Execution: &pb.Execution{Context: proto.Clone(c).(*pb.Context), State: pb.ExecutionState_EXECUTION_STATE_MEASURING, RegisteredAt: now.Format(time.RFC3339Nano), RegistrationOrder: fmt.Sprintf("%020d", s.Next)}, LastData: now, Cutoff: now, Ancestors: ancestors, Push: c.PullRequest == 0 && c.BaselineCommit == "", Baselines: map[string]Baseline{}}
	if !r.Push {
		r.Base.Commit = base
		if b := s.BaselineRepresentative(base, now); b != nil {
			r.Base.Execution = b.Execution.Context.ExecutionId
		}
	}
	s.Records[c.ExecutionId] = r
	return r
}
func key(dataset, row string) string { return fmt.Sprintf("%d:%s%s", len(dataset), dataset, row) }
func find(r *Record, dataset, row string) (*pb.Dataset, *pb.Row) {
	if r != nil {
		for _, m := range r.Execution.Measurements {
			if m.Dataset.Id == dataset {
				for _, v := range m.Rows {
					if v.Id == row {
						return m.Dataset, v
					}
				}
				return m.Dataset, nil
			}
		}
	}
	return nil, nil
}
func (s *State) baseline(r *Record, dataset, row string) Baseline {
	if !r.Push {
		return r.Base
	}
	for _, sha := range r.Ancestors {
		b := s.BaselineRepresentative(sha, r.Cutoff)
		_, v := find(b, dataset, row)
		if v != nil {
			return Baseline{b.Execution.Context.ExecutionId, sha}
		}
	}
	return Baseline{}
}

// IsUploadRetry recognizes stored rows without changing state. Upload still
// validates lifecycle, tombstones and the complete request before returning it.
func (s *State) IsUploadRetry(c *pb.Context, d *pb.Dataset, rows []*pb.Row) bool {
	r := s.Records[c.ExecutionId]
	if r == nil || d == nil || len(rows) == 0 || !proto.Equal(c, r.Execution.Context) {
		return false
	}
	for _, row := range rows {
		if row == nil {
			return false
		}
		oldD, old := find(r, d.Id, row.Id)
		if old == nil || !proto.Equal(oldD, d) || !equalValues(old, row) {
			return false
		}
	}
	return true
}

// Upload validates the whole request before mutating state. The database owns
// the repository lock across this call and commit, including completion races.
func (s *State) Upload(c *pb.Context, d *pb.Dataset, rows []*pb.Row, now time.Time, ancestors []string, base string) (*Record, error) {
	if s.DeletedExecutions[c.ExecutionId] {
		return nil, errors.New("execution was deleted")
	}
	if e := Validate(d, rows); e != nil {
		return nil, e
	}
	if len(rows) == 0 {
		return nil, errors.New("upload requires at least one row")
	}
	r := s.Records[c.ExecutionId]
	if r != nil {
		if r.RemovedDatasets[d.Id] {
			return nil, errors.New("dataset was deleted from this execution")
		}
		if !proto.Equal(c, r.Execution.Context) {
			return nil, errors.New("execution context conflicts")
		}
		if r.Execution.State != pb.ExecutionState_EXECUTION_STATE_MEASURING || !now.Before(r.LastData.Add(6*time.Hour)) {
			return nil, errors.New("execution is closed")
		}
	}
	added := false
	for _, row := range rows {
		oldD, old := find(r, d.Id, row.Id)
		if oldD != nil && !proto.Equal(oldD, d) {
			return nil, errors.New("dataset configuration conflicts")
		}
		if old != nil {
			if !equalValues(old, row) {
				return nil, errors.New("row " + row.Id + ": values conflict")
			}
		} else {
			added = true
		}
	}
	if r == nil {
		r = s.Register(c, now, ancestors, base)
	} else if len(ancestors) > len(r.Ancestors) {
		// First-parent ancestry is immutable. Later jobs may need a longer verified
		// prefix for rows skipped by earlier ancestors; existing row pins stay frozen.
		for i, sha := range r.Ancestors {
			if ancestors[i] != sha {
				return nil, errors.New("Git ancestry changed")
			}
		}
		r.Ancestors = ancestors
	}
	var m *pb.Measurement
	for _, item := range r.Execution.Measurements {
		if item.Dataset.Id == d.Id {
			m = item
		}
	}
	if m == nil {
		m = &pb.Measurement{Dataset: proto.Clone(d).(*pb.Dataset)}
		r.Execution.Measurements = append(r.Execution.Measurements, m)
	}
	for _, row := range rows {
		_, old := find(r, d.Id, row.Id)
		if old == nil {
			r.Baselines[key(d.Id, row.Id)] = s.baseline(r, d.Id, row.Id)
			m.Rows = append(m.Rows, proto.Clone(row).(*pb.Row))
		}
	}
	if added {
		r.LastData = now
	}
	return r, nil
}
func equalValues(a, b *pb.Row) bool {
	if len(a.Values) != len(b.Values) {
		return false
	}
	for k, v := range a.Values {
		x, e := Parse(v)
		if e != nil {
			return false
		}
		y, e := Parse(b.Values[k])
		if e != nil || x.Cmp(y) != 0 {
			return false
		}
	}
	return true
}
func (s *State) Complete(c *pb.Context, now time.Time) (*Record, error) {
	if s.DeletedExecutions[c.ExecutionId] {
		return nil, errors.New("execution was deleted")
	}
	r := s.Records[c.ExecutionId]
	if r == nil {
		r = s.Register(c, now, nil, "")
	}
	if !proto.Equal(c, r.Execution.Context) {
		return nil, errors.New("execution context conflicts")
	}
	if r.Execution.State == pb.ExecutionState_EXECUTION_STATE_COMPLETED {
		return r, nil
	}
	if r.Execution.State == pb.ExecutionState_EXECUTION_STATE_INTERRUPTED || !now.Before(r.LastData.Add(6*time.Hour)) {
		return nil, errors.New("execution is interrupted")
	}
	r.Execution.State = pb.ExecutionState_EXECUTION_STATE_COMPLETED
	r.Execution.CompletedAt = now.Format(time.RFC3339Nano)
	return r, nil
}
func (s *State) Comparisons(r *Record, exact *Record, direct bool) []*pb.Comparison {
	out := []*pb.Comparison{}
	for _, m := range r.Execution.Measurements {
		for _, row := range m.Rows {
			ref := r.Baselines[key(m.Dataset.Id, row.Id)]
			b := s.Records[ref.Execution]
			if direct {
				b = exact
				ref = Baseline{}
				if b != nil {
					ref = Baseline{b.Execution.Context.ExecutionId, b.Execution.Context.Commit}
				}
			}
			bd, br := find(b, m.Dataset.Id, row.Id)
			for _, c := range m.Dataset.Columns {
				var bc *pb.Column
				var bv string
				if br != nil {
					for _, v := range bd.Columns {
						if v.Id == c.Id {
							bc = v
							bv = br.Values[c.Id]
						}
					}
				}
				v := Compare(c, bc, row.Values[c.Id], bv)
				if b != nil && b.RemovedDatasets[m.Dataset.Id] {
					v.Status = pb.ComparisonStatus_COMPARISON_STATUS_BASELINE_DELETED
				}
				v.DatasetId = m.Dataset.Id
				v.RowId = row.Id
				v.BaselineExecutionId = ref.Execution
				v.BaselineCommit = ref.Commit
				if b == nil {
					v.Status = pb.ComparisonStatus_COMPARISON_STATUS_UNAVAILABLE
					if ref.Execution != "" {
						v.Status = pb.ComparisonStatus_COMPARISON_STATUS_BASELINE_DELETED
					}
				}
				out = append(out, v)
			}
		}
	}
	sort.Slice(out, func(i, j int) bool {
		a, b := out[i], out[j]
		return key(a.DatasetId, a.RowId)+a.ColumnId < key(b.DatasetId, b.RowId)+b.ColumnId
	})
	return out
}

// BaselineCandidates limits Git traversal to rows with eligible stored data.
// A first upload with no possible baseline requires no history walk.
func (s *State) BaselineCandidates(dataset *pb.Dataset, rows []*pb.Row, cutoff time.Time) map[string]map[string]bool {
	result := map[string]map[string]bool{}
	if dataset == nil {
		return result
	}
	for _, record := range s.Records {
		c := record.Execution.Context
		if c.PullRequest != 0 || record.Execution.State != pb.ExecutionState_EXECUTION_STATE_COMPLETED {
			continue
		}
		if s.BaselineRepresentative(c.Commit, cutoff) != record {
			continue
		}
		for _, row := range rows {
			if row == nil {
				continue
			}
			_, found := find(record, dataset.Id, row.Id)
			if found != nil {
				if result[c.Commit] == nil {
					result[c.Commit] = map[string]bool{}
				}
				result[c.Commit][row.Id] = true
			}
		}
	}
	return result
}
