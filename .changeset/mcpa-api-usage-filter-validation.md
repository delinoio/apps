---
"@servers/mcpa-api": patch
---

Usage and call-record filters now reject invalid model names and unknown provider or status numbers with an `INVALID_INPUT` error that names the field, instead of failing with a dependency error or matching the wrong rows.
