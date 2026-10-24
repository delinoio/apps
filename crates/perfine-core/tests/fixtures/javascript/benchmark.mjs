// SPDX-License-Identifier: Apache-2.0
import { countPairs } from "./kernel.mjs";
const size = Number(process.argv[2]);
if (![128, 1024].includes(size)) throw new Error("Expected size 128 or 1024");
const values = Uint8Array.from({length: size}, (_, i) => (i * 17 + Math.floor(i / 7)) % 256);
// Keep normal V8 JIT. Warm-up is excluded from timing but included in CPU profiles.
let checksum = 0;
for (let i = 0; i < 2000; i++) checksum += countPairs(values);
const iterations = 2000;
const start = process.hrtime.bigint();
for (let i = 0; i < iterations; i++) checksum += countPairs(values);
const elapsed = process.hrtime.bigint() - start;
console.log(JSON.stringify({iterations, elapsed_ns: Number(elapsed), ns_per_op: Number(elapsed) / iterations, checksum}));
