# Repository working instructions

## Instruction and contract ownership

- Ordinary feature additions, bug fixes, tests and behavior-contract changes do not by themselves require an AGENTS.md update. Update instructions only when development procedures, directory ownership or repository/domain development rules change. Keep feature requirements in the owning contract; do not duplicate them in AGENTS.md.
- Do not add issue-specific feature paragraphs to instruction files. Read the owning project index and domain contracts before implementation.
- Record new implementation status, validation results and unresolved limits in pull requests, Linear issues and CI logs/artifacts. Include the source revision, commands, results and unperformed checks; distinguish fixtures, builds and packaging from actual native/account/platform acceptance. Exclude secrets, user state and raw native content. Preserve existing validation records.

## Task navigation

- List `docs/` files, then use [the documentation catalog](docs/README.md) to find the owning project index and domain contracts. Read contracts for the affected paths and cross-domain consumers; the catalog is not a requirement to read every contract.
- Read scoped `AGENTS.md` files from the repository root through each affected directory before editing.
- Keep the complete project/domain document inventory in `docs/README.md`. Group project-specific entries under their project heading and shared contracts under repository sections. The directory map below provides entry points, not a second complete catalog.

## Instructions

- Do not add issue or pull-request links or identifying numbers, including Linear `DELI-<number>` identifiers, to repository instruction files or documentation. Describe requirements directly and link to the owning repository-local contracts. Domain, service and technical-documentation links remain allowed; record issue/PR traceability in pull requests, Linear issues or CI records instead.

- Use the `@docs/` directory as the source of truth for project contracts and implementation documents.
- License repository-owned source and future distributions under Apache-2.0, except `servers/`, which uses FSL-1.1-ALv2 with an Apache-2.0 future license. Keep imported code and bundled fonts under their original licenses with notices intact; follow `docs/repository-license-contract.md`.
- All repository-wide rules must be defined in the appropriate AGENTS.md.
- Every repository-owned directory named `dist` is ignored generated output and must never be tracked. Generate required `dist` content explicitly before compilation, testing, or packaging, and remove generated `dist` directories from the final worktree.
- Track repository asset files of at least 512 KiB through exact-path Git LFS attributes. Checkouts that compile, test, or package those assets must hydrate LFS objects first; preserve source licenses and notices. Follow `docs/repository-workflow-contract.md`.
- Keep `docs/` up-to-date.
- For documentation authoring and editing tasks, do not arbitrarily omit, delete, or simplify requested or source-backed content; if content, scope, or intent is ambiguous, ask the user before deciding what to remove, merge, or reinterpret; update or create the relevant `AGENTS.md` in the same change only when development procedures, directory ownership or repository/domain development rules change.
- Public documentation surfaces must not document repository-internal implementation details. Keep internal source-of-truth contracts, architecture notes, repo-local paths, and operational internals in `docs/`; curate public docs in their owning application around user-facing behavior, supported workflows, stable public interfaces, and maintainer-facing paths only when those paths are explicitly part of the public contract.
- Write all code and comments in English.
- Write Linear issue titles and explanatory body text in English. Preserve exact code, commands, paths, diagnostic messages, search queries, direct quotations and localized UI strings when their original text is required; explain non-English evidence in English. Translate existing non-English issue prose without changing requirements, evidence, references, attachments or issue state.
- When introducing a workaround, leave sufficient comments that explain why it exists, its scope, and the conditions for removing it.
- Prefer enum types over strings whenever possible.
- If you modified Rust code and a root `Cargo.toml` workspace exists, run `cargo test` from the root directory before finishing your task. The repository has no Rust workspace yet; the first crate change creates it and adds the crate as an explicit member in the same change, as described in `crates/AGENTS.md`.
- If you modified frontend code, run `pnpm test` from the root directory before finishing your task; only `apps/delino-accounts` defines frontend tests today, so `apps/delino-docs` and `apps/delino-site` are covered by their `typecheck` and `build` scripts instead.
- Commit your work as frequent as possible using git. Do NOT use `--no-verify` flag.
- Run `git commit` only after `git add`; once files are staged, commit without unnecessary delay so staged changes are preserved in history.
- Committing may require workspace binaries (for example, git hooks). If required binaries are missing, run `pnpm install` at the repository root and retry the commit.
- CI changes must preserve affected manual validation and exclude macOS runners from PRs, main pushes and affected manual runs. Follow `docs/repository-workflow-contract.md`.
- Keep `.infisical.json`, `.dev-environment/`, real `.env` files, user credentials, and production/release/signing credentials uncommitted. Service-local `.env.example` files contain names, validation guidance, placeholders, and safe loopback defaults only; do not add a root environment example or expose internal secret paths in public product docs.
- After addressing pull request review comments and pushing updates, mark the corresponding review threads as resolved.
- When no explicit scope is specified and you are currently working within a pull request scope, interpret instructions within the current pull request scope.
- Do not guess; rather search for the web.
- Follow `docs/repository-dependency-security-contract.md` for dependency security updates, validation evidence, and unresolved upstream constraints. Keep unresolved advisories visible; a fixed runtime pin or an unavailable patch is not a vulnerability fix.
- All Go code belongs to the single root Go module `github.com/delinoio/mono` defined in root `go.mod`. Do not add nested `go.mod` or `go.work` files.
- Prioritize Connect RPC-based communication for business flows over Tauri-specific bindings.
- Prefer React Query for frontend server-state management when it is available.
- When using React Query with Connect RPC, use `@connectrpc/connect-query` from `https://github.com/connectrpc/connect-query-es`.
- When accessing `github.com`, use the GitHub CLI (`gh`) instead of browser-based workflows when possible.
- Run GitHub CLI (`gh`) commands outside sandbox restrictions by default; use the required approval flow when escalation is needed.
- If an operation is blocked by sandbox restrictions, retry it without sandbox restrictions using the required approval flow.

## Monorepo Structure Map

- `docs/`: Source of truth for project contracts and repository documentation.
- `apps/`: User-facing apps and documentation web surfaces.
- `crates/`: Rust crates and Rust-based tooling.
- `cmds/`: Go command tools for workflow orchestration.
- `servers/`: Backend services, licensed under FSL-1.1-ALv2.
- `protos/`: Versioned protocol schemas and committed Go bindings.
- `packages/`: Shared generated and runtime packages.
- `packaging/`: Package-manager template assets for release automation.
- `.agents/skills/`: Workspace-local Codex skills and reusable agent workflows.
- `.claude/skills`: Tracked symlink to `.agents/skills/` so Claude Code loads the same skills. Add or edit skills only in `.agents/skills/`.
- `scripts/`: Shared development, CI, installation and release tooling.
- `.github/`: GitHub workflows and shared actions.

## Canonical Directory Map

- `docs/README.md`: Complete documentation catalog and project onboarding navigation.
- `docs/repository-defaults.md`: Default technology choices.
- `docs/repository-workflow-contract.md`: Installation, formatting and CI.
- `docs/repository-environment-contract.md`: Configuration and secrets.
- `docs/repository-license-contract.md`: Licensing and imported material.
- `docs/repository-dependency-security-contract.md`: Dependency security validation.
- `docs/repository-issue-tracker-contract.md`: Issue tracking and pull request references.
- `docs/repository-auth-contract.md`: Shared Logto authentication and user identity.
- `docs/repository-billing-contract.md`: Billing currency and monetary precision.
- `docs/repository-e2e-contract.md`: Browser end-to-end tests for web apps.
- `docs/project-template.md`: Project index template.
- `docs/domain-template.md`: Domain contract template.

## Repository Default Technology Choices

- Follow `docs/repository-defaults.md` when a more specific project or domain contract does not choose a different approach.
- New persisted entities should use UUID v7 identifiers by default unless a documented compatibility, storage, protocol, or product issue requires another ID shape.
- AI-based search should default to Cloudflare AI Search unless a project contract documents a different backend and migration boundary.
- When a new project does not specify its primary language, default to Golang.
- Prefer Rspack-family build tools when possible, including Rsbuild and Rspress for app and documentation surfaces.
- Static sites under `apps/` should use Rsbuild/Rspress-style toolchains and deploy to Cloudflare Pages by default. Existing documented exceptions remain valid until their project contract changes.
- File handling should default to Cloudflare R2 object storage plus signed URLs for upload and download access unless a project contract documents another storage or access pattern.

## Documentation-First Policy

- New project creation requires `docs/project-<id>.md` and at least one `docs/<domain>-<project-or-component>-<contract>.md` before runtime implementation.
- Every structural change to project paths must update the corresponding project index and relevant domain contract docs in the same change.
- Development-procedure, directory-ownership and repository/domain development-rule updates must be written in the appropriate `AGENTS.md` in the same change.
- Domain-level `AGENTS.md` files must route to their owning contracts and follow the instruction-update policy.
- Documentation-only project onboarding may use `planned` paths, but runtime implementation must not begin before canonical paths are created and documented.
- When user-facing documentation content changes, update the owning public documentation pages in the same change set as needed.
- If a project splits into multiple deployables, the project index must include path ownership and integration boundaries, and component-level domain docs must exist.

## New Project Onboarding Checklist

- Reserve a unique `project-id`.
- Create project path skeleton and add `.gitkeep` if implementation is not started.
- Add `docs/project-<project-id>.md` using `docs/project-template.md`.
- Add at least one domain contract doc using `docs/domain-template.md`.
- Documentation-only phase may mark canonical paths as `planned` before creating path skeletons; create the skeleton and add explicit workspace membership in the same change where Rust runtime implementation begins.
- Update root and domain `AGENTS.md` files when directory ownership or development rules change.
- Ensure path and naming contracts are consistent across docs and AGENTS rules.

## Naming Rules

- Use lowercase kebab-case for project IDs and directory names unless runtime conventions require otherwise.
- Use `project-` prefix for project index docs.
- Use domain prefixes (`apps-`, `cmds-`, `servers-`, `crates-`, `protos-`, `packages-`) for domain contract docs.
- Use `repository-<topic>-contract.md` for repository-level contract docs that span project or domain ownership.
- Use enum-like canonical identifiers in documents where values must remain stable.

## Issue Tracker Contract

- Use the Linear team `DELI` (Mono) at https://linear.app/delinoio/team/DELI/ as the primary issue tracker for all development work, including bugs, features, tasks, chores, investigations and review, QA or analysis follow-ups.
- Use GitHub Issues in this repository for customer support only. Do not create GitHub issues for development work.
- In pull request descriptions, add a standalone `Closes DELI-<number>` line for each fully resolved Linear issue and a standalone `Refs DELI-<number>` line for each partially addressed or related Linear issue. Do not use both for the same issue.
- Follow `docs/repository-issue-tracker-contract.md`.

## Linear Issue Style Contract

- Apply this contract to all open/new Linear issues in the `DELI` team.
- Use issue titles in the format `<domain>: <description>`.
- `<domain>` must use stable lowercase identifiers from project/domain contracts (for example: `tooling`, `docs`).
- `<description>` should be concise, specific, and start with a lowercase verb phrase when possible.
- Do not use bracket-style project prefixes like `[project]`.
- Use the following Markdown section order for issue bodies:
  - `## Summary`
  - `## Evidence`
  - `## Current Gap`
  - `## Proposed Scope`
  - `## Acceptance Criteria`
  - `## Test Scenarios`
  - `## Out of Scope`
- Optional `## Additional Notes` may be appended only when needed.

## Authentication Rules

- Logto is the only identity provider. All services share one Logto tenant per deployment environment; do not add service-local password storage, sign-up flows or another identity provider.
- Use the Logto user ID (`sub`) as the canonical user identifier across services.
- Backend services must validate Logto access tokens (signature, issuer, expiration, audience and required scopes) and must not accept ID tokens as API credentials.
- Use `delino-accounts` for account management in every service. Link users to `https://accounts.delino.io` for profile and account settings, and read shared account data from `https://accounts.api.delino.io`. Do not build service-local account management pages or duplicate the shared account profile.
- Follow `docs/repository-auth-contract.md` and `docs/project-delino-accounts.md`.

## Billing Rules

- Charge users in USD only.
- Store and compute prices, usage costs, credits and balances with 0.000001 USD (micro-USD) precision as signed 64-bit integers named with a `_usd_micros` suffix. Never use binary floating-point types for monetary values.
- Document rounding at payment-provider boundaries and in sub-micro-USD calculations in the owning domain contract; round only for display.
- Use `delino-accounts` for billing in every service. `delino-accounts` owns payment methods, payment-provider integration, balances, credits, charges and invoices. Other services report billable usage or charges to `https://accounts.api.delino.io` and link users to `https://accounts.delino.io` for billing management. Do not integrate a payment provider directly or keep a service-local balance.
- Follow `docs/repository-billing-contract.md` and `docs/project-delino-accounts.md`.

## GitHub Pull Request Title Contract

- Apply this contract to newly created pull requests.
- Pull request titles must use Conventional Commit-style format with a required scope: `<type>(<scope>): <description>`.
- `<type>` must be an appropriate Conventional Commit type such as `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `ci`, `build`, `perf`, or `revert`.
- `<scope>` must use a stable lowercase project, component, domain, or tooling identifier from repository contracts when one applies (for example: `tooling`, `docs`, `ci`).
- Do not create unscoped pull request titles or use bracket-style project prefixes like `[project]`.

## Runtime Baselines

- Root `.nvmrc` is the canonical Node.js runtime selector for local development workflows.
- The current required runtime is Node.js `24` (LTS major line).
- When bumping the runtime baseline, update `.nvmrc` and relevant CI/runtime docs in the same change set.

## Frontend Design Rules

- Frontend work in `apps/` must follow Toss Design Guidelines for UX/UI decisions across web and mobile surfaces.
- If a form has a single critical input, that input must receive focus when the form is shown.
- Dialog UIs must support closing with the `Esc` key.

## Shell Command Safety Rules

- Use `$(...)` for command substitution; do not use legacy backticks in new scripts.
- Wrap all file paths in quotes by default in shell commands and scripts to prevent whitespace and glob-expansion bugs.
- Apply strict quoting and escaping for all dynamic shell values to prevent command injection and parsing bugs.

## Formatting Rules

- Define workspace build, development, lint, typecheck and test commands in the owning package and run them through root Turborepo scripts. Keep repository-wide formatting and Git hook setup as direct commands; use the root `test:tooling` task for shared formatter and hook fixtures. Follow `docs/repository-workflow-contract.md` for task configuration and cache boundaries.
- Root `pnpm dev` must start all workspace development servers. Each package that owns a development server must expose a `dev` script; do not restrict the root command with package or affected filters.
- Use the pinned Rstack CLI through root `pnpm format` and `pnpm format:check` for changed JS/TS, CSS, JSON, YAML and Markdown files. Keep shared options and exclusions in `rstack.config.ts`.
- The Lefthook pre-commit hook must automatically format staged Rstack-supported files and stage the fixes in the same commit while preserving unstaged edits. Use the shared formatter's local `--staged --write` mode; keep CI checks on the exact planner comparison.
- CI must check the exact planner comparison without rewriting sources or expanding to a repository-wide formatting migration. Add the owning Rust, Go and Protobuf checks when those source domains are introduced. Follow `docs/repository-workflow-contract.md`.

## Logging Rules

- Debug by logging. Write sufficient logs to support debugging, incident analysis, and operational troubleshooting.
- Prefer structured logging over ad-hoc plain text logs for business and system events.
- Go code should use `log/slog` (or a compatible structured logger built on it).
- Rust code should use `tracing` (or a compatible structured logging facade).
- CLI and operator-facing logs should enable ANSI color by default; allow opt-out with documented flags or environment variables.

## Contract navigation

- [Documentation catalog](docs/README.md) lists project indexes and domain contracts. Project indexes own canonical paths, stable project IDs and cross-domain invariants.
- [Repository workflow](docs/repository-workflow-contract.md) owns CI, release, allocation-validation records and validation procedures.
- [Environment ownership](docs/repository-environment-contract.md) owns development selectors, service configuration, child environments and shutdown.
- Read scoped instructions in every affected domain, including cross-domain consumers.
