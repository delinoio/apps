---
"@servers/mcpa-api": patch
---

Return a typed, content-free invalid_argument error for Connect request bodies that fail to decode. Custom handler codecs mark the failed decode in-band so that the management interceptor renders mcpa.v1.ErrorDetail (INVALID_INPUT, trace_id equal to X-Mcpa-Request-Id) and logs one rejection line, instead of connect-go quoting the offending text in an untyped error.
