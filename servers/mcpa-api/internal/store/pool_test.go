// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"testing"
	"time"
)

func TestTimeoutMillisRoundsUpAndNeverReturnsZero(t *testing.T) {
	for _, tc := range []struct {
		in   time.Duration
		want int64
	}{
		{5 * time.Second, 5000},
		{time.Millisecond, 1},
		{1500 * time.Microsecond, 2},
		{time.Nanosecond, 1},
		{0, 1},
	} {
		if got := timeoutMillis(tc.in); got != tc.want {
			t.Errorf("timeoutMillis(%s) = %d, want %d", tc.in, got, tc.want)
		}
	}
}
