---
"@servers/mcpa-api": patch
---

Admit a personal-key management check that Delcore denies only with `BLOCK_REASON_DELETING`, so personal keys of a deleting tenant can still view records and remove credentials and keys as the management tenant state rule requires. Other denial reasons, tenant or user mismatches, paid checks and model calls are unchanged.
