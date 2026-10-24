// SPDX-License-Identifier: Apache-2.0
#include "kernel.h"
uint64_t count_pairs(const uint8_t *values, size_t n) {
    uint64_t pairs = 0;
    for (size_t i = 0; i < n; ++i)
        for (size_t j = i + 1; j < n; ++j)
            pairs += values[i] == values[j];
    return pairs;
}
