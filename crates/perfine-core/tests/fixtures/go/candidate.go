// SPDX-License-Identifier: Apache-2.0
package pairs

//go:noinline
func CountPairs(values []byte) uint64 {
	var counts [256]uint64
	var pairs uint64
	for _, value := range values {
		pairs += counts[value]
		counts[value]++
	}
	return pairs
}
