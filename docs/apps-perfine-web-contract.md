# Perfine dashboard and approved UI

## Scope

`apps/perfine` owns the dashboard at `https://perfine.delino.io`. The authoritative approved text below governs behavior; preview artwork supplies visual context only. Public usage documentation remains separate from these internal contracts.

## Runtime and Language

TypeScript/React with Rsbuild and shared `packages/ui` primitives, deployed to Cloudflare Pages. This browser UI is an explicit exception to the non-server Go default. Use Connect RPC with `@connectrpc/connect-query` for React Query server state.

## Users and Operators

Authorized personal/organization space readers, experiment operators, publishers, and managers. All results remain private even for public repositories.

## Interfaces and Contracts

### Dashboard and approved design

Dashboard: `https://perfine.delino.io`. API: `https://perfine.api.delino.io`.

The user approved the preview and the following authoritative text specification. Artwork is supporting evidence; behavior below governs implementation. Example repositories, SHA values, numbers, and character counters in the preview are not actual measurements or additional product limits.

- Navigation: space selection, repositories, experiments, settings. Flow: connect/setup, browse experiments, inspect a result, inspect GitHub diff, review publication, approve a draft PR.
- Desktop reference viewport is 1440px; mobile reference is 390px. Use shared neutral UI primitives, clear hierarchy, restrained primary actions, and readable status labels. Desktop emphasizes lists and detail; mobile stacks cards and uses a separate review view rather than compressing a desktop table.
- Detail shows execution state separately from performance verdict, base/head SHA, metric and unit, per-case comparison, and evidence completeness. Label `Ir` as instruction count rather than elapsed time.
- Provide space/repository selection; connection setup; history, comparisons, evidence, and status; cloud start/stop/resume; external GitHub diff navigation; exact SHA/content approval and publication; manager deletion/disconnection and raw-artifact consent.
- Execution forms require all declared limits and policies. Settings must not contain AI credential inputs. UI visibility never substitutes for server authorization.
- Publication review shows target repository, base, head SHA, title, and body before `Approve and create draft PR`. A change in approved target/base/SHA/material PR content requires new approval. Remeasure when measured production content changes, not merely because PR prose was edited. This text supersedes the preview's overly broad re-experiment message.
- Display loading, empty, failed, incomplete/inconclusive, permission-denied, cancelled, and cloud-unavailable states with reasons and recovery actions. No candidates is not an error. Unsupported cloud execution is disabled with a local CLI alternative.
- English UI, CLI, and user documentation. Do not restrict source-code or user-input language.
- Support keyboard use, visible focus, screen-reader names, status labels beyond color, dialog Escape dismissal and focus restoration. Autofocus a form's single critical input. Verify responsive navigation and review behavior.
- Preserve draft-only publication, external diff viewing, private results, and credential boundaries. No automatic merge, payment UI, anonymous sharing, or hosted source editor.

## Storage

Authoritative experiment state and evidence live in the API, not the browser. Fetch eligible downloads using server-issued 15-minute signed URLs. Do not expose anonymous result sharing or store complete repositories/candidate diffs in the service.

## Security

Use Logto identity and Delcore spaces. Match independent read/experiment/publish/manage actions to server authorization. Never provide AI credential inputs. [Api contract](servers-perfine-api-contract.md) owns authority checks, consent, deletion, and retained URL limitations; [Cli contract](cmds-perfine-cli-contract.md) owns approval binding.

## Logging

Use request IDs to correlate API errors. Do not put secrets, raw profiles/test logs, or candidate source into UI telemetry. Display actionable failure and recovery reasons.

## Build and Test

After frontend implementation, run `pnpm test` from `apps/perfine`. Verify the approved 1440px desktop and 390px mobile layouts, separate mobile review, keyboard/focus/Escape behavior, screen-reader names and non-color status, every declared state, external diffs, limits and policy forms, approval invalidation, and absent credential/billing/source-edit controls.

Automated verification is the first-release gate. Follow the acceptance criteria and test scenarios in the [project index](project-perfine.md). Record source revision, commands, results, and unperformed checks in PR/CI evidence. Fixtures, mocked integrations, builds, and packaging do not establish live or native acceptance.

## Dependencies and Integrations

[Protocol contract](protos-perfine-v1-contract.md), [Api contract](servers-perfine-api-contract.md), [Engine contract](crates-perfine-core-contract.md), [shared UI](packages-ui-foundation.md), and [environment ownership](repository-environment-contract.md). Follow repository Toss Design Guidelines for implementation decisions within the approved specification.

## Change Triggers

Update the project index and affected owning contracts together when ownership, interfaces, evidence, or policy changes. Update scoped instructions only for ownership or development-procedure changes; follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Perfine project](project-perfine.md)
- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Repository license](repository-license-contract.md)

## Browser integration

The app uses the generated Perfine v1 services and `@connectrpc/connect-query`.
Logto supplies bearer tokens; Delcore `GetCurrentUser` supplies paginated personal
and organization spaces. There is no anonymous result route. Query state is
scoped by space/resource and cleared on sign-out. Permission failures hide cached
results. Manage, experiment and publish controls remain independent of read.

Repository setup uses an operator-configured GitHub App installation URL and
explicit repository/installation IDs and a masked GitHub access token input.
The token field spans both desktop columns and stacks with the form on mobile.
Its helper explains that the token verifies repository administrator access and
is not saved. Send it only in the connection request's `x-github-token` header;
never retain it in mutation variables, browser storage, or shared transport state.
Clear the input when dispatching and require re-entry after failure. Disable
submission while pending and preserve the manage permission gate.
The API resolves IDs and verifies both
management and GitHub administrator authority. Connections and histories use
cursor pagination. Evidence links are issued on demand, after server checks.

Execution setup uses editable source SHA, language, input-hash, workload-case,
frequency/protection, correctness-check, workload confirmation, tool/environment,
limit and policy fields. Empty defaults must not invent source or evidence.
`PrepareExperiment` computes authoritative fingerprints and returns the immutable
specification and engine configuration for review and local use. Preparation is
available to authorized experiment operators even without an eligible cloud tool.
Optional JSON import seeds editable settings and still requires fresh preparation.
The browser never asks users to calculate configuration fingerprints. Changes
invalidate the prepared result and confirmation, including stale in-flight replies.
Start submits the reviewed authoritative specification; started runs stay immutable.

Engine v1 uncertainty uses near-threshold margin and repeat tolerance for Ir,
and also maximum relative sample range and decision margin for declared policies.
A sample range is not a confidence interval. Measurements-only mode makes no
pass claim. Detail and publication review show the exact frozen policy without
deprecated confidence-interval summaries. The API validates all settings independently.

Publication has a separate review view at both widths. Confirmation resets on
content edits. The client compares the returned approval binding with the exact
submitted draft, then sends that binding to `PublishDraft`. It polls retained
approval/publication records on uncertain outcomes without creating another
approval or automatically resubmitting publication. Source changes require
remeasurement; prose-only edits require new approval, not new measurements.

## App-local configuration and checks

`pnpm dev` in `apps/perfine` injects the root Infisical development configuration,
then registers the Portless name `perfine` with fixed upstream
`127.0.0.1:46324`. Port conflicts fail. Root orchestration and CI selection belong
to the repository workflow; this app does not start API servers or databases.

Public browser settings are `PUBLIC_LOGTO_ENDPOINT`, `PUBLIC_LOGTO_APP_ID`,
`PUBLIC_DELCORE_API_URL`, `PUBLIC_DELCORE_RESOURCE`, `PUBLIC_PERFINE_API_URL`,
`PUBLIC_PERFINE_RESOURCE` and `PUBLIC_PERFINE_GITHUB_APP_URL`. The Perfine resource
defaults to the Delcore resource, matching existing service identity patterns.
Configure `/callback` and the sign-out origin in Logto. Cross-origin APIs must
permit the exact frontend origin and authorization headers. Only public client
configuration may enter the frontend; service credentials remain server-owned.

`pnpm test` runs TypeScript and mocked typed-RPC interaction tests. `pnpm build`
produces the Pages bundle and license notices. `tests/browser.config.ts` is a
separate mocked-RPC browser entry for responsive inspection; it is not imported
by the production entry and does not bypass production authentication. Start it
only for verification with `pnpm exec rsbuild dev --config tests/browser.config.ts`.
It displays an explicit mock-data banner. These checks do not establish live
Logto, Delcore, GitHub, Actions, R2 or backend acceptance. Remove generated `dist`
output after validation.
