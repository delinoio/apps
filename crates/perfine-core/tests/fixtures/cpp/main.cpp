// SPDX-License-Identifier: Apache-2.0
#include "kernel.h"
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    char *end = NULL;
    unsigned long n = strtoul(argv[1], &end, 10);
    if (*end || n > 4096) return 2;
    uint8_t values[4096];
    for (size_t i = 0; i < n; ++i) values[i] = (uint8_t)((i * 17 + i / 7) % 256);
    uint64_t checksum = 0;
    // Separate compilation without LTO keeps the kernel calls observable.
    for (size_t iteration = 0; iteration < 100; ++iteration)
        checksum += count_pairs(values, n);
    printf("%llu\n", (unsigned long long)checksum);
    return 0;
}
