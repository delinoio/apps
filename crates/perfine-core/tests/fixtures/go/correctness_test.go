// SPDX-License-Identifier: Apache-2.0
package pairs

import "testing"

func TestIndependentExamples(t *testing.T) {
	repeated, alphabet := make([]byte, 100), make([]byte, 256)
	for i := range repeated {
		repeated[i] = 7
	}
	for i := range alphabet {
		alphabet[i] = byte(i)
	}
	for _, test := range []struct {
		values   []byte
		expected uint64
	}{
		{nil, 0}, {[]byte{0}, 0}, {[]byte{1, 2, 3}, 0},
		{[]byte{255, 0, 255, 0, 255}, 4}, {repeated, 4950}, {alphabet, 0},
	} {
		if got := CountPairs(test.values); got != test.expected {
			t.Fatalf("got %d, want %d", got, test.expected)
		}
	}
}
