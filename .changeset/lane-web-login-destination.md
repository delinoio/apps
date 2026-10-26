---
"@apps/lane": patch
---

Send the selected task or repository destination and its optional account guard to
`/auth/login` as separate typed fields next to the query-free registered `return_to`,
and deliver the restored destination once after the session is installed. The default
sign-in link used for account switching stays query-free.

<!-- public-changelog:start -->

Opening a Lane web notification or repository link while signed out now takes you to
the linked task or repository after you sign in. If you sign in with a different
account than the link was meant for, Lane explains the mismatch instead of opening it.

<!-- public-changelog:end -->
