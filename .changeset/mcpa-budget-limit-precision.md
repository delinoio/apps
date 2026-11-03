---
"@servers/mcpa-api": patch
---

Accept budget limits as canonical non-negative decimals of up to 64 characters instead of 12 integer digits and 6 fraction digits. Limits are stored and returned unrounded, and the exhaustion check compares exact decimals.
