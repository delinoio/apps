# Review and approve a draft pull request

Performance evaluation and publication are separate actions. Review correctness, incremental and cumulative comparisons, per-case regressions, exceptions, limitations, and the actual production diff before approval. Keep independent candidates separate.

A publisher can approve their own candidate; a second reviewer is not required. Hosted publication requires the independent publish grant and a current human identity. An API key does not substitute for human approval.

## Local CLI publication

Use GitHub CLI authentication with authority for the target and head repositories. Push the exact measured candidate SHA separately and explicitly; `publish` does not push it for you. Do not include temporary harnesses, fork CI infrastructure, unrelated commits, or uncommitted user work in the production change.

Create a JSON draft file with `schema_version: 1`, `target_repository`, `base_ref`, `head_repository`, `head_ref`, `title`, and `body`. Repository values use `owner/repository`. Use the connected repository or an authorized fork head and upstream target as appropriate.

```sh
perfine publish --experiment "$PERFINE_EXPERIMENT_ID"   --candidate "$PERFINE_CANDIDATE_ID" --draft "draft.json"
```

Review the returned content, diff, evaluation, and approval fingerprint. Set `PERFINE_APPROVAL_FINGERPRINT` to that exact returned fingerprint only after reviewing it, then approve:

```sh
perfine publish --experiment "$PERFINE_EXPERIMENT_ID"   --candidate "$PERFINE_CANDIDATE_ID" --draft "draft.json"   --approve "$PERFINE_APPROVAL_FINGERPRINT"
```

If policy requires explicit human judgment, supply the reason through `--human-judgment` during the reviewed publication flow. That does not waive correctness, evidence, or source-correspondence checks, and does not supply missing policy-exception review evidence.

## Exact approval

Approval binds the target repository, base, exact candidate SHA, evidence, production diff, title, and body. The dashboard review shows the binding before **Approve and create draft PR**. Diffs open externally; there is no hosted source editor.

If the target, base, SHA, or material PR content changes, review and approve again. Prose-only edits require new approval but not new measurements. If a rebase or conflict resolution changes production content, remeasure the actual submission source.

Publication creates a draft only. Perfine never merges automatically. If a create request has an uncertain outcome, reconcile it against GitHub before retrying; do not submit another draft manually to work around uncertainty.

The current hosted verifier accepts a single isolated commit limited to measured profile-covered production paths. It rejects renamed files, unrelated commits, infrastructure paths, and truncated comparisons. A broader diff is not automatically publishable merely because its benchmark improved.
