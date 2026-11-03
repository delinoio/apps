---
"@servers/mcpa-api": patch
---

Return `UsageSummary.total_gib` and `estimated_usd` as exact canonical decimals (up to 30 and 32 fraction digits) instead of values rounded to six places. Console display rounding is unchanged and stays in the console.
