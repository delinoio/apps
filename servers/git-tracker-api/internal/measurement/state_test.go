// SPDX-License-Identifier: FSL-1.1-ALv2

package measurement

import (
	pb "github.com/delinoio/apps/protos/gittracker/v1"
	"testing"
	"time"
)

func fixture() (*pb.Dataset, []*pb.Row) {
	return &pb.Dataset{Id: "size", Name: "Size", Columns: []*pb.Column{{Id: "original", Name: "Original", Unit: "B", Direction: pb.Direction_DIRECTION_LOWER, AbsoluteThreshold: "0"}}}, []*pb.Row{{Id: "file", Values: map[string]string{"original": "9007199254740993.0000000000000001"}}}
}
func TestExactComparison(t *testing.T) {
	c := &pb.Column{Id: "x", Unit: "B", Direction: pb.Direction_DIRECTION_LOWER, AbsoluteThreshold: "0"}
	if Compare(c, c, "9007199254740993.0000000000000001", "9007199254740993").Status != pb.ComparisonStatus_COMPARISON_STATUS_REGRESSION {
		t.Fatal("precision lost")
	}
	for _, x := range []struct {
		value, base, abs, pct string
		want                  pb.ComparisonStatus
	}{
		{"110", "100", "10", "10", pb.ComparisonStatus_COMPARISON_STATUS_CHANGED},
		{"111", "100", "20", "10", pb.ComparisonStatus_COMPARISON_STATUS_REGRESSION},
		{"1", "0", "", "1", pb.ComparisonStatus_COMPARISON_STATUS_CHANGED},
		{"1", "0", "0.5", "1", pb.ComparisonStatus_COMPARISON_STATUS_REGRESSION},
		{"-9", "-10", "", "9", pb.ComparisonStatus_COMPARISON_STATUS_REGRESSION},
	} {
		c.AbsoluteThreshold = x.abs
		c.PercentageThreshold = x.pct
		if got := Compare(c, c, x.value, x.base); got.Status != x.want {
			t.Fatalf("%+v: %+v", x, got)
		}
	}
	b := &pb.Column{Id: "x", Unit: "KiB"}
	c.AbsoluteThreshold = "0"
	c.PercentageThreshold = ""
	if Compare(c, b, "1024", "1").Status != pb.ComparisonStatus_COMPARISON_STATUS_UNCHANGED {
		t.Fatal("unit conversion")
	}
	c.Direction = pb.Direction_DIRECTION_NEUTRAL
	if Compare(c, c, "2", "1").Status == pb.ComparisonStatus_COMPARISON_STATUS_REGRESSION {
		t.Fatal("neutral regression")
	}
	for _, v := range []string{"NaN", "Infinity", "1/3", "01", "1e999999999"} {
		if _, e := Parse(v); e == nil {
			t.Fatal(v)
		}
	}
}
func TestAtomicAndLifecycle(t *testing.T) {
	s := NewState()
	now := time.Now().UTC()
	d, rows := fixture()
	c := &pb.Context{ExecutionId: "a", Commit: "a"}
	r, e := s.Upload(c, d, rows, now, nil, "")
	if e != nil {
		t.Fatal(e)
	}
	_, e = s.Upload(c, d, rows, now.Add(time.Hour), nil, "")
	if e != nil || !r.LastData.Equal(now) {
		t.Fatal("retransmission advanced clock")
	}
	bad := []*pb.Row{{Id: "new", Values: map[string]string{"original": "1"}}, {Id: "file", Values: map[string]string{"original": "2"}}}
	if _, e = s.Upload(c, d, bad, now, nil, ""); e == nil {
		t.Fatal("conflict accepted")
	}
	if _, v := find(r, "size", "new"); v != nil {
		t.Fatal("partial save")
	}
	if n := s.Interrupt(now.Add(6 * time.Hour)); n != 1 {
		t.Fatal(n)
	}
	if _, e = s.Complete(c, now.Add(6*time.Hour)); e == nil {
		t.Fatal("completed interrupted")
	}
	c = &pb.Context{ExecutionId: "empty", Commit: "a"}
	r, e = s.Complete(c, now)
	if e != nil {
		t.Fatal(e)
	}
	done := r.Execution.CompletedAt
	if _, e = s.Complete(c, now.Add(time.Hour)); e != nil || r.Execution.CompletedAt != done {
		t.Fatal("completion changed")
	}
	if _, e = s.Upload(c, d, rows, now, nil, ""); e == nil {
		t.Fatal("upload after complete")
	}
}
func TestFrozenBaselinesAndRegistration(t *testing.T) {
	s := NewState()
	now := time.Now().UTC()
	d, rows := fixture()
	upload := func(id, commit string, at time.Time, parents []string) *Record {
		t.Helper()
		r, e := s.Upload(&pb.Context{ExecutionId: id, Commit: commit}, d, rows, at, parents, "")
		if e != nil {
			t.Fatal(e)
		}
		return r
	}
	a := upload("a", "base", now, nil)
	s.Complete(a.Execution.Context, now)
	old := upload("old", "target", now.Add(time.Second), []string{"skip", "base"})
	newer := upload("new", "target", now.Add(2*time.Second), []string{"skip", "base"})
	s.Complete(newer.Execution.Context, now.Add(3*time.Second))
	s.Complete(old.Execution.Context, now.Add(4*time.Second))
	if s.Representative("target", time.Time{}) != newer {
		t.Fatal("late older completion won")
	}
	historical := upload("late", "skip", now.Add(5*time.Second), nil)
	s.Complete(historical.Execution.Context, now.Add(6*time.Second))
	got := s.Comparisons(old, nil, false)
	if got[0].BaselineExecutionId != "a" {
		t.Fatal("late history changed baseline")
	}
	delete(s.Records, "a")
	if s.Comparisons(old, nil, false)[0].Status != pb.ComparisonStatus_COMPARISON_STATUS_BASELINE_DELETED {
		t.Fatal("deleted baseline reconstructed")
	}
	if s.Comparisons(old, nil, true)[0].Status != pb.ComparisonStatus_COMPARISON_STATUS_UNAVAILABLE {
		t.Fatal("direct comparison used ancestors")
	}
}

func TestPRUnavailableAndLaterRowsKeepCutoff(t *testing.T) {
	s := NewState()
	now := time.Now().UTC()
	d, rows := fixture()
	pr := &pb.Context{ExecutionId: "pr", Commit: "head", PullRequest: 1}
	r, e := s.Upload(pr, d, rows, now, nil, "base")
	if e != nil {
		t.Fatal(e)
	}
	base := &pb.Context{ExecutionId: "base", Commit: "base"}
	if _, e = s.Upload(base, d, rows, now.Add(time.Second), nil, ""); e != nil {
		t.Fatal(e)
	}
	s.Complete(base, now.Add(time.Second))
	if s.Comparisons(r, nil, false)[0].Status != pb.ComparisonStatus_COMPARISON_STATUS_UNAVAILABLE {
		t.Fatal("missing PR baseline changed")
	}
	push := &pb.Context{ExecutionId: "push", Commit: "target"}
	p, e := s.Upload(push, d, rows, now.Add(2*time.Second), []string{"late", "base"}, "")
	if e != nil {
		t.Fatal(e)
	}
	other := []*pb.Row{{Id: "later-job", Values: map[string]string{"original": "1"}}}
	late := &pb.Context{ExecutionId: "late", Commit: "late"}
	s.Upload(late, d, other, now.Add(3*time.Second), nil, "")
	s.Complete(late, now.Add(3*time.Second))
	if _, e = s.Upload(push, d, other, now.Add(4*time.Second), nil, ""); e != nil {
		t.Fatal(e)
	}
	for _, v := range s.Comparisons(p, nil, false) {
		if v.RowId == "later-job" && v.BaselineExecutionId != "" {
			t.Fatal("late job used later cutoff")
		}
	}
}

func TestDeletionDoesNotResurrectMeasurements(t *testing.T) {
	s := NewState()
	s.DeletedExecutions = map[string]bool{"deleted": true}
	d, rows := fixture()
	c := &pb.Context{ExecutionId: "deleted"}
	if _, e := s.Upload(c, d, rows, time.Now(), nil, ""); e == nil {
		t.Fatal("deleted execution recreated by upload")
	}
	if _, e := s.Complete(c, time.Now()); e == nil {
		t.Fatal("deleted execution recreated by completion")
	}
	c.ExecutionId = "active"
	r, e := s.Upload(c, d, rows, time.Now(), nil, "")
	if e != nil {
		t.Fatal(e)
	}
	r.RemovedDatasets = map[string]bool{d.Id: true}
	r.Execution.Measurements = nil
	if _, e = s.Upload(c, d, rows, time.Now(), nil, ""); e == nil {
		t.Fatal("deleted dataset recreated in same execution")
	}
}

func TestPRMeasurementsCannotReplaceUpstreamBaselines(t *testing.T) {
	s := NewState()
	now := time.Now().UTC()
	d, rows := fixture()
	upstream := &pb.Context{ExecutionId: "upstream", Commit: "base", Branch: "main"}
	s.Upload(upstream, d, rows, now, nil, "")
	s.Complete(upstream, now)
	fork := &pb.Context{ExecutionId: "fork", Commit: "base", Branch: "main", PullRequest: 7}
	s.Upload(fork, d, rows, now.Add(time.Second), nil, "base")
	s.Complete(fork, now.Add(time.Second))
	target := &pb.Context{ExecutionId: "target", Commit: "next", Branch: "main"}
	r, e := s.Upload(target, d, rows, now.Add(2*time.Second), []string{"base"}, "")
	if e != nil {
		t.Fatal(e)
	}
	if s.Comparisons(r, nil, false)[0].BaselineExecutionId != "upstream" {
		t.Fatal("fork changed upstream baseline")
	}
	pr := &pb.Context{ExecutionId: "pr", Commit: "head", Branch: "main", PullRequest: 8}
	r, e = s.Upload(pr, d, rows, now.Add(3*time.Second), nil, "base")
	if e != nil {
		t.Fatal(e)
	}
	if s.Comparisons(r, nil, false)[0].BaselineExecutionId != "upstream" {
		t.Fatal("fork changed PR merge-base source")
	}
}

func TestConfigurationFreezeAndHigherDirection(t *testing.T) {
	s := NewState()
	d, rows := fixture()
	c := &pb.Context{ExecutionId: "first", Commit: "one"}
	now := time.Now()
	if _, e := s.Upload(c, d, rows, now, nil, ""); e != nil {
		t.Fatal(e)
	}
	d.Name = "Renamed dataset"
	if _, e := s.Upload(c, d, rows, now, nil, ""); e == nil {
		t.Fatal("execution configuration changed")
	}
	s.Complete(c, now)
	next := &pb.Context{ExecutionId: "second", Commit: "two"}
	r, e := s.Upload(next, d, rows, now.Add(time.Second), []string{"one"}, "")
	if e != nil {
		t.Fatal(e)
	}
	if s.Comparisons(r, nil, false)[0].BaselineExecutionId != "first" {
		t.Fatal("rename lost history")
	}
	col := &pb.Column{Id: "speed", Unit: "ops/s", Direction: pb.Direction_DIRECTION_HIGHER, AbsoluteThreshold: "0"}
	if Compare(col, col, "9", "10").Status != pb.ComparisonStatus_COMPARISON_STATUS_REGRESSION {
		t.Fatal("higher direction did not detect regression")
	}
	if Compare(col, col, "11", "10").Status != pb.ComparisonStatus_COMPARISON_STATUS_IMPROVEMENT {
		t.Fatal("higher direction did not detect improvement")
	}
	incompatible := &pb.Column{Id: "speed", Unit: "s"}
	if Compare(col, incompatible, "9", "10").Status != pb.ComparisonStatus_COMPARISON_STATUS_INCOMPATIBLE {
		t.Fatal("incompatible units compared")
	}
}
