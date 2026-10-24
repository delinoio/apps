// SPDX-License-Identifier: Apache-2.0
export function countPairs(values) {
  const counts = new Uint32Array(256);
  let pairs = 0;
  for (const value of values) {
    pairs += counts[value];
    counts[value]++;
  }
  return pairs;
}
