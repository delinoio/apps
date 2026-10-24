// SPDX-License-Identifier: Apache-2.0
package pairs

import "testing"

var sink uint64

func BenchmarkPairs(b *testing.B) {
	for _, test := range []struct {
		name string
		n    int
	}{{"bulk", 1024}, {"protected", 128}} {
		b.Run(test.name, func(b *testing.B) {
			values := make([]byte, test.n)
			for i := range values {
				values[i] = byte((i*17 + i/7) % 256)
			}
			b.ResetTimer()
			for i := 0; i < b.N; i++ {
				sink = CountPairs(values)
			}
		})
	}
}
