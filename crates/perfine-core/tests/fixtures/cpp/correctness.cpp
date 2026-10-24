// SPDX-License-Identifier: Apache-2.0
#include "kernel.h"
#include <assert.h>
int main(void) {
    const uint8_t singleton[] = {0};
    const uint8_t distinct[] = {1, 2, 3};
    const uint8_t mixed[] = {255, 0, 255, 0, 255};
    uint8_t repeated[100], alphabet[256];
    for (size_t i = 0; i < 100; ++i) repeated[i] = 7;
    for (size_t i = 0; i < 256; ++i) alphabet[i] = (uint8_t)i;
    assert(count_pairs(singleton, 0) == 0);
    assert(count_pairs(singleton, 1) == 0);
    assert(count_pairs(distinct, 3) == 0);
    assert(count_pairs(mixed, 5) == 4);
    assert(count_pairs(repeated, 100) == 4950);
    assert(count_pairs(alphabet, 256) == 0);
    return 0;
}
