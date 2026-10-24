// SPDX-License-Identifier: Apache-2.0
export function countPairs(values) {
  let pairs = 0;
  for (let i = 0; i < values.length; i++) {
    for (let j = i + 1; j < values.length; j++) pairs += Number(values[i] === values[j]);
  }
  return pairs;
}
