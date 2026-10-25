# Private hosted spaces and uploads

Local operation requires no account. The optional hosted API at `https://perfine.api.delino.io` uses your Delcore personal or organization space and Logto sign-in. There is no separate Perfine membership system or CLI browser-login command.

## Independent permissions

| Grant      | Permitted operations                                                               |
| ---------- | ---------------------------------------------------------------------------------- |
| Read       | View results and eligible evidence.                                                |
| Experiment | Upload, start, resume, and stop experiments, subject to availability.              |
| Publish    | Approve exact pull request content and publish a draft.                            |
| Manage     | Connect/disconnect repositories, control raw-evidence consent, and delete results. |

No grant implies another. A manager does not automatically have experiment or publish permission. Every operation checks current authority. All hosted results are private to authorized space readers, even for public repositories. There is no anonymous result sharing.

A repository connects to one space at a time. Connection requires both Perfine management permission and GitHub repository administrator authority, with GitHub App access to that repository. Reconnecting in the same space reuses history. Connecting to another space starts new history; old private data stays in its original space.

## Source and credential boundaries

Hosting accepts eligible measurements, metadata, profiles, and correctness results. It does not store whole repositories or candidate diffs. Inspect diffs locally or on GitHub.

Raw profiles or test logs can contain source snippets only if a manager approved those artifact types for that repository before the run. Consent applies to subsequent runs. Revocation blocks new raw uploads, including uploads from previously approved runs. Source consent never permits credentials or secrets: remove them or reject the upload. Keep ineligible originals locally or on GitHub, and disclose missing hosted evidence.

AI credentials stay with your locally authenticated AI CLI. For a future eligible cloud adapter, supported unattended authentication must remain entirely inside your repository's Actions environment. Perfine's server never receives, proxies, or stores AI credentials. Upload envelopes do not accept agent authentication configuration. Candidate build/test/measurement receives no AI or publication credentials.

## Upload and manual resend

The CLI builds the evidence bundle, normalized observations, raw artifact references, and manifest checksum from the retained local experiment. You do not need to prepare an upload envelope. Upload is explicit; a local run is not automatically uploaded to hosting.

Set `DELCORE_API_KEY` securely in your local environment with the exact space and Perfine experiment grant. With UUID v7 request, experiment, space, and connection IDs, run:

```sh
perfine --request-id "$PERFINE_REQUEST_ID" upload \
  --experiment "$PERFINE_EXPERIMENT_ID" \
  --endpoint "https://perfine.api.delino.io" \
  --space "$PERFINE_SPACE_ID" \
  --connection "$PERFINE_CONNECTION_ID"
```

Add `--candidate "$PERFINE_CANDIDATE_ID"` to select a candidate. For repository-approved raw evidence, supply the applicable `--consent-revision`; the server verifies that consent predates the run and is still valid. A revision number does not grant consent. Never paste a key into a command, committed file, or support report.

Upload failure preserves local results and durable upload progress. Resolve eligibility, authority, size, or quota errors before resending. For an unchanged interrupted upload, reuse the same UUID v7 request ID. The CLI reuses the retained bundle and checks upload state before continuing. Do not replace accepted evidence or change frozen inputs.

Hosting inspects supported raw profiles, including bounded gzip pprof, and rejects unknown binary formats or secrets. Frozen configuration containing environment variables is ineligible for hosting. Keep rejected originals locally. Partial uploads remain incomplete and cannot support a hosted pass. Space storage, single-upload, request, and cloud-concurrency limits can reject new work; authorized read/delete access remains available.

For custom integrations, `perfine.v1.ExperimentService.PrepareExperiment` accepts typed configuration and returns a specification with authoritative fingerprints. Preparation does not start an experiment or enable unavailable cloud execution. The manifest digest covers deterministic protobuf bytes, not transport JSON. Use the versioned engine uncertainty fields; deprecated confidence-interval fields are rejected.

Actions evidence uses scoped GitHub Actions OIDC rather than exporting your local API key. The actor must have the required current authority; running a workflow is not sufficient. An API key is not a human approval and cannot dispatch hosted publication.

## Retention and deletion

Experiments and evidence have no automatic product-data expiry; retain them until deletion. Logs, audits, and request deduplication have a separate 30-day lifetime from first creation. Retries do not extend that period.

Managers can delete results. Accepted deletion immediately blocks new reads, new download links, late uploads, and resume. Physical removal is asynchronous and failed cleanup is retried until complete. Deleted experiment IDs remain reserved; use a new ID for new work.

Signed download URLs last 15 minutes. Previously issued links can remain usable until expiry after deletion or permission revocation. Downloaded copies cannot be recalled. Replacement links require current read authority.

Disconnecting a repository does not delete its evidence. App removal, disconnection, or loss of execution authority blocks new jobs, uploads, and publication and requests cancellation of running Actions. If cancellation fails, use the direct GitHub run link and cancel there with current repository authority. A cancellation request is not confirmation that the run stopped.
