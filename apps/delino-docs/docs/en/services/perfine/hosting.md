# Private hosted spaces and uploads

Local operation requires no account. Optional hosting at [perfine.delino.io](https://perfine.delino.io) uses your Delcore personal or organization space and Logto sign-in. There is no separate Perfine membership system or CLI browser-login command.

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

AI credentials stay with your locally authenticated AI CLI. For a future eligible cloud adapter, supported unattended authentication must remain entirely inside your repository's Actions environment. Perfine's server never receives, proxies, or stores AI credentials. Neither dashboard configuration nor upload envelopes accept agent authentication configuration. Candidate build/test/measurement receives no AI or publication credentials.

## Upload and manual resend

The current CLI requires an externally prepared v1 envelope. It does not automatically prepare configuration, construct an evidence bundle, or encode its manifest. Do not assume that a local run automatically appears in the dashboard.

Configuration preparation uses the public `perfine.v1.ExperimentService.PrepareExperiment` interface. Supply typed configuration and all limits; preparation returns the immutable specification with calculated fingerprints. Preparation alone does not start an experiment or make cloud execution available. A prepared specification expires after 30 days. Do not calculate fingerprints by hashing transport JSON.

For an integration that already provides the envelope, it contains `specification` and `manifest`. Use the final v1 metric, hardware, and uncertainty fields, including `metric.engineKind` and `environment.hardware`. The manifest digest covers deterministic protobuf bytes, not JSON. Preserve exact artifact bytes and hashes. Deprecated confidence-interval fields are not supported uncertainty inputs.

Set `DELCORE_API_KEY` securely in your local environment with the exact space and Perfine experiment grant. Do not paste a key into a command, file to be committed, or support report. With the corresponding IDs and prepared file, run:

```sh
perfine --request-id "$PERFINE_REQUEST_ID" upload   --experiment "$PERFINE_EXPERIMENT_ID"   --envelope "upload-envelope.json"   --endpoint "https://perfine.api.delino.io"   --space "$PERFINE_SPACE_ID"   --connection "$PERFINE_CONNECTION_ID"
```

Upload failure preserves local results. Resolve the reported eligibility, authority, size, or quota problem before resending. For an unchanged interrupted upload, reuse the same UUID v7 request ID and immutable envelope. Do not replace accepted evidence or change frozen inputs. A corrected artifact set needs a newly eligible manifest.

Hosting currently rejects opaque binary artifacts without a privacy parser. A native profile can therefore remain local-only even when local evaluation succeeds. Partial uploads remain incomplete and cannot support a hosted pass. Space storage limits, single-upload limits, request limits, and cloud concurrency limits can reject new work; authorized read/delete access remains available.

Actions evidence uses scoped GitHub Actions OIDC rather than exporting your local API key. The actor must have the required current authority; running a workflow is not sufficient. An API key is not a human approval and cannot dispatch hosted publication.

## Retention and deletion

Experiments and evidence have no automatic product-data expiry; retain them until deletion. Logs, audits, and request deduplication have a separate 30-day lifetime from first creation. Retries do not extend that period.

Managers can delete results. Accepted deletion immediately blocks new reads, new download links, late uploads, and resume. Physical removal is asynchronous and failed cleanup is retried until complete. Deleted experiment IDs remain reserved; use a new ID for new work.

Signed download URLs last 15 minutes. Previously issued links can remain usable until expiry after deletion or permission revocation. Downloaded copies cannot be recalled. Replacement links require current read authority.

Disconnecting a repository does not delete its evidence. App removal, disconnection, or loss of execution authority blocks new jobs, uploads, and publication and requests cancellation of running Actions. If cancellation fails, use the direct GitHub run link and cancel there with current repository authority. A cancellation request is not confirmation that the run stopped.
