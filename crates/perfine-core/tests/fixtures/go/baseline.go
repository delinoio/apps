// SPDX-License-Identifier: Apache-2.0
package pairs

//go:noinline
func CountPairs(values []byte) uint64 {
	var pairs uint64
	for i, value := range values {
		for _, other := range values[i+1:] {
			if value == other {
				pairs++
			}
		}
	}
	return pairs
}
