---
"@servers/lane-api": patch
---

Resolve synchronized repository aliases to the canonical repository in the webhook
domain. Receiver creation and saving and new events persist the canonical repository;
receiver and event projections and the repository filters of `ListReceivers` and
`ListEvents` treat an alias and its canonical ID as the same repository with shared
cursor scope. Repository deletion now selects current receiver owners and original event
owners across all aliases, so signing keys, event payloads, leases, processing records
and saved response and change content are erased. `lane-api migrate` runs an idempotent,
per-account compatibility cleanup for webhook resources that earlier builds left behind
for an already deleted repository. Entity versions, fingerprints, sender deduplication
and replay identities are unchanged.

<!-- public-changelog:start -->

Deleting a Lane repository now also removes the webhook receivers and events that were
connected to it through a synchronized duplicate of that repository, and filtering
webhook receivers and events by either repository ID shows the same results.

<!-- public-changelog:end -->
