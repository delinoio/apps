---
"@servers/lane-api": patch
---

Build the receiver endpoint from an origin-only `LANE_WEBHOOK_BASE_URL` as
`<origin>/webhooks/<receiver-id>` so signed deliveries reach intake. An explicit
non-root path is still the complete intake base, so `/webhooks` and `/webhooks/` give a
single prefix and `/hooks` is unchanged. Receivers, keys and stored replay receipts are
not rewritten; senders configured from an old origin-relative endpoint need `/webhooks`
inserted before the receiver ID.

<!-- public-changelog:start -->

New Lane webhook receivers show an endpoint that accepts signed deliveries. If a sender
uses an endpoint issued earlier that lacks `/webhooks`, add `/webhooks` before the
receiver ID in its configuration; the signing key does not change.

<!-- public-changelog:end -->
