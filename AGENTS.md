# Repository working instructions

## Instruction and contract ownership

- Ordinary feature additions, bug fixes, tests and behavior-contract changes do not by themselves require an AGENTS.md update. Update instructions only when development procedures, directory ownership or repository/domain development rules change. Keep feature requirements in the owning contract; do not duplicate them in AGENTS.md.
- Do not add issue-specific feature paragraphs to instruction files. Read the owning project index and domain contracts before implementation.
- Record new implementation status, validation results and unresolved limits in pull requests, issues and CI logs/artifacts. Include the source revision, commands, results and unperformed checks; distinguish fixtures, builds and packaging from actual native/account/platform acceptance. Exclude secrets, user state and raw native content. Preserve existing validation records.

## Instructions

- Do not add issue or pull-request links or identifying numbers to repository instruction files or documentation. Describe requirements directly and link to the owning repository-local contracts. Domain, service and technical-documentation links remain allowed; record issue/PR traceability in pull requests, issues or CI records instead.

- Use the `docs/` directory as the source of truth for project contracts and implementation documents.
- License repository-owned files under `servers/` and their distributions under FSL-1.1-ALv2. Repository-owned source outside `servers/` and its distributions use Apache-2.0. Keep imported code and bundled fonts under their original licenses with notices intact; follow `docs/repository-license-contract.md`.
- All repository-wide rules must be defined in the appropriate AGENTS.md.
- Every repository-owned directory named `dist` is ignored generated output and must never be tracked. Generate required `dist` content explicitly before compilation, testing, or packaging, and remove generated `dist` directories from the final worktree.
- Track repository asset files of at least 512 KiB through exact-path Git LFS attributes. Checkouts that compile, test, or package those assets must hydrate LFS objects first; preserve source licenses and notices. Schema-only Buf baseline clones must skip LFS smudging within the breaking-check command so pointer-only checkouts need no unrelated asset downloads. Follow `docs/repository-workflow-contract.md`.
- List files in `docs/` before starting each task, and keep `docs/` up-to-date.
- For documentation authoring and editing tasks, do not arbitrarily omit, delete, or simplify requested or source-backed content; if content, scope, or intent is ambiguous, ask the user before deciding what to remove, merge, or reinterpret; update or create the relevant `AGENTS.md` in the same change only when development procedures, directory ownership or repository/domain development rules change.
- Public documentation surfaces must not document repository-internal implementation details. Keep internal source-of-truth contracts, architecture notes, repo-local paths, and operational internals in `docs/`; curate any public documentation surfaces around user-facing behavior, supported workflows, stable public interfaces, and maintainer-facing paths only when those paths are explicitly part of the public contract.
- Write all code and comments in English.
- Write GitHub issue titles and explanatory body text in English. Preserve exact code, commands, paths, diagnostic messages, search queries, direct quotations and localized UI strings when their original text is required; explain non-English evidence in English. Translate existing non-English issue prose without changing requirements, evidence, references, attachments or issue state.
- When introducing a workaround, leave sufficient comments that explain why it exists, its scope, and the conditions for removing it.
- Prefer enum types over strings whenever possible.
- If you modified Rust code, run `cargo test` from the root directory before finishing your task.
- If you modified frontend code, run `pnpm test` from the frontend directory before finishing your task.
- Commit your work as frequent as possible using git. Do NOT use `--no-verify` flag.
- Run `git commit` only after `git add`; once files are staged, commit without unnecessary delay so staged changes are preserved in history.
- Committing may require workspace binaries (for example, git hooks). If required binaries are missing, run `pnpm install` at the repository root and retry the commit.
- FixKey releases must not require a native-acceptance revision variable or comparison. Keep immutable source validation, signing and protected publication gates in the owning release workflow.
- CI changes must preserve default affected manual validation and exclude macOS runners from PRs, main pushes and affected manual runs. Do not add a full manual CI mode; retain owning release platform gates and follow `docs/repository-workflow-contract.md`.
- Development `dev` and `dev:*` scripts must inject Infisical development variables through the local root project configuration. Follow [environment ownership](docs/repository-environment-contract.md) for CLI setup and secret boundaries.
- Run root `pnpm dev` to start workspace development through process-compose, or root `pnpm dev:portless` to expose the frontends and Go APIs through named local URLs. For one app, run `pnpm dev` from the owning app to serve its frontend or `pnpm dev:desktop` to launch its native shell. Keep the documented fixed loopback ports and fail on conflicts. Follow the [repository workflow](docs/repository-workflow-contract.md) and owning application contracts.
- Root `pnpm dev` must wait for the local Compose databases and pass their resolved URLs only to API development launchers, which apply them after Infisical injection. Keep service-local environment files unchanged. Follow the [development Compose procedure](docs/repository-workflow-contract.md#local-development-databases), keep schema migrations explicit and run application development commands on the host.
- Follow [Turborepo remote caching](docs/repository-workflow-contract.md#turborepo-remote-caching) for local cache setup, CI access and build hash inputs. Keep Turbo credentials and linked team configuration uncommitted.
- Run root `pnpm i18n:translate` or `pnpm i18n:check` to process all workspaces that provide the corresponding localization command. Allow bounded parallel translation generation while serializing shared ChatGPT authentication updates and per-app output writes. Follow the [localization contract](docs/apps-delino-site-localization-contract.md).
- Keep `.infisical.yaml`, legacy `.infisical.json`, `.dev-environment/`, real `.env` files, user credentials, and production/release/signing credentials uncommitted. Service-local `.env.example` files contain names, validation guidance, placeholders, and safe loopback defaults only; do not add a root environment example or expose internal secret paths in public product docs.
- After addressing pull request review comments and pushing updates, mark the corresponding review threads as resolved.
- When no explicit scope is specified and you are currently working within a pull request scope, interpret instructions within the current pull request scope.
- Do not guess; rather search for the web.
- Follow `docs/repository-dependency-security-contract.md` for dependency security updates, validation evidence, and unresolved upstream constraints. Keep unresolved advisories visible; a fixed runtime pin or an unavailable patch is not a vulnerability fix.
- Prioritize Connect RPC-based communication for business flows over Tauri-specific bindings.
- Prefer React Query for frontend server-state management when it is available.
- When using React Query with Connect RPC, use `@connectrpc/connect-query` from `https://github.com/connectrpc/connect-query-es`.
- When accessing `github.com`, use the GitHub CLI (`gh`) instead of browser-based workflows when possible.
- Run GitHub CLI (`gh`) commands outside sandbox restrictions by default; use the required approval flow when escalation is needed.
- If an operation is blocked by sandbox restrictions, retry it without sandbox restrictions using the required approval flow.

## Repository ownership and integrated development

- This repository owns clients, native shells, protocols, consumer examples and server implementations. `servers/` owns runtimes, background workers, migrations and OCI packaging; `protos/` owns schemas and generated Go bindings.
- Read `docs/repository-boundary-contract.md` before changing a boundary. Keep project and domain contracts in root `docs/`; read scoped instructions for every affected domain.
- Develop client, protocol and server changes in the same checkout and review. The root Go module is `github.com/delinoio/apps`; server packages consume local protocol packages. Do not create a companion checkout, protocol dependency pin, local workspace or duplicate generated source for this integration.
- Run applicable client, protocol and server checks. Use disposable databases and mocked providers for server integration; production credentials and live-account acceptance remain separate.

## Monorepo Structure Map

- `servers/`: Delcore, DevBase and Git Tracker services, migrations, OCI packaging and the independent server Cargo workspace; follow `servers/AGENTS.md`.
- `servers/crates/`: shared server Rust modules under FSL-1.1-ALv2. Keep these outside the root client Cargo workspace.
- `packages/ui`: Shared React UI primitives; follow `packages/AGENTS.md`.
- `docs/`: Source of truth for project contracts and repository documentation.
- `apps/devbase`: DevBase web application; its API server lives in `servers/devbase-api`.
- `apps/delino-site`: Delino web application.
- `apps/delino-docs`: Public documentation app for Delino services.
- `apps/delidev`: DeliDev desktop shell and native host.
- `apps/delitools`: DeliTools desktop/mobile/web application and mobile release helpers.
- `apps/fixkey-app`: Standalone FixKey desktop product and native shell.
- `crates/fixkey-core`: FixKey correction engine and platform adapters.
- `protos/delcore/v1`: Delcore schemas, generated Go and TypeScript messages and Connect bindings.
- `protos/devbase/v1`: DevBase schemas, generated Go and TypeScript messages and Connect bindings.
- `scripts/`: Shared build and development support.
- `scripts/localization/`: Local ChatGPT translation generation and offline validation for Delino Site and Delino Docs; follow its scoped instructions.
- `scripts/ci/`: Affected selection, validation and evidence tooling.
- `examples/delcore-client`: Go consumer example.
- `examples/devbase-client` and `examples/devbase-client-ts`: DevBase Go and TypeScript consumer examples.
- `.github/`: CI actions and workflows, including mobile release automation.

## Canonical Directory Map

Use the [documentation catalog](docs/README.md) for the complete list of repository contracts, project indexes, domain contracts and templates. Project indexes own canonical paths and integration boundaries; keep the detailed document list in the catalog.

## Repository Default Technology Choices

- Follow `docs/repository-defaults.md` when a more specific project or domain contract does not choose a different approach.
- New persisted entities should use UUID v7 identifiers by default unless a documented compatibility, storage, protocol, or product issue requires another ID shape.
- AI-based search should default to Cloudflare AI Search unless a project contract documents a different backend and migration boundary.
- Default to Rust for new server implementations unless the owning project or domain contract specifies another language. For other new projects without a specified primary language, default to Golang.
- Prefer tools from the Rspack and Rstack families when possible, including Rsbuild and Rspress for app and documentation surfaces.
- Static sites under `apps/` should use Rsbuild/Rspress-style toolchains and deploy to Cloudflare Pages by default. Existing documented exceptions remain valid until their project contract changes.
- File handling should default to Cloudflare R2 object storage plus signed URLs for upload and download access unless a project contract documents another storage or access pattern.

## Documentation-First Policy

- New project creation requires `docs/project-<id>.md` and at least one `docs/<domain>-<project-or-component>-<contract>.md` before runtime implementation.
- Every structural repository change, including changes to project paths, must update the corresponding project index and relevant domain contract docs in the same change.
- Development-procedure, directory-ownership and repository/domain development-rule updates must be written in the appropriate `AGENTS.md` in the same change.
- Domain-level `AGENTS.md` files must route to their owning contracts and follow the instruction-update policy.
- When user-facing documentation content changes, update the relevant public documentation pages in the same change set as needed. `apps/delino-docs` owns the public documentation app; root `docs/` retains internal contracts.
- If a project splits into multiple deployables, the project index must include path ownership and integration boundaries, and component-level domain docs must exist.

## New Project Onboarding Checklist

- Reserve a unique `project-id`.
- Create project path skeleton and add `.gitkeep` if implementation is not started.
- Add `docs/project-<project-id>.md` using `docs/project-template.md`.
- Add at least one domain contract doc using `docs/domain-template.md`.
- Documentation-only phase may mark canonical paths as `planned` before creating path skeletons; create and document the skeleton before runtime implementation begins, and add explicit workspace membership in the same change where Rust runtime implementation begins.
- Update root and domain `AGENTS.md` files when directory ownership or development rules change.
- Ensure path and naming contracts are consistent across docs and AGENTS rules.

## Naming Rules

- Use lowercase kebab-case for project IDs and directory names unless runtime conventions require otherwise.
- Use `project-` prefix for project index docs.
- Use domain prefixes (`apps-`, `cmds-`, `servers-`, `crates-`, `protos-`, `packages-`) for domain contract docs.
- Use `repository-<topic>-contract.md` for repository-level contract docs that span project or domain ownership.
- Use enum-like canonical identifiers in documents where values must remain stable.

## GitHub Issue Style Contract

- Apply this contract to all open/new GitHub issues.
- Use issue titles in the format `<domain>: <description>`.
- `<domain>` must use stable lowercase identifiers from project/domain contracts (for example: `delcore`, `delidev`, `delitools`).
- `<description>` should be concise, specific, and start with a lowercase verb phrase when possible.
- Do not use bracket-style project prefixes like `[delcore]`.
- Use the following Markdown section order for issue bodies:
  - `## Summary`
  - `## Evidence`
  - `## Current Gap`
  - `## Proposed Scope`
  - `## Acceptance Criteria`
  - `## Test Scenarios`
  - `## Out of Scope`
- Optional `## Additional Notes` may be appended only when needed.

## GitHub Pull Request Title Contract

- Apply this contract to newly created pull requests.
- Pull request titles must use Conventional Commit-style format with a required scope: `<type>(<scope>): <description>`.
- `<type>` must be an appropriate Conventional Commit type such as `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `ci`, `build`, `perf`, or `revert`.
- `<scope>` must use a stable lowercase project, component, domain, or tooling identifier from repository contracts when one applies (for example: `delcore`, `delidev`, `delitools`, `docs`, `ci`).
- Do not create unscoped pull request titles or use bracket-style project prefixes like `[delcore]`.

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

- Use the pinned Rstack CLI through root `pnpm format` and `pnpm format:check` for changed JS/TS, CSS, JSON, YAML and Markdown files. Keep shared options and exclusions in `rstack.config.ts`.
- The Lefthook pre-commit hook must automatically format staged Rstack-supported files and stage the fixes in the same commit while preserving unstaged edits. Use the shared formatter's local `--staged --write` mode; run CI formatting checks across all tracked supported files.
- CI formatting must run without change detection and check all tracked supported files without rewriting sources. Preserve Rust, Go and Protobuf format checks. Follow `docs/repository-workflow-contract.md`.

## Logging Rules

- Debug by logging. Write sufficient logs to support debugging, incident analysis, and operational troubleshooting.
- Prefer structured logging over ad-hoc plain text logs for business and system events.
- Go code should use `log/slog` (or a compatible structured logger built on it).
- Rust code should use `tracing` (or a compatible structured logging facade).
- CLI and operator-facing logs should enable ANSI color by default; allow opt-out with documented flags or environment variables.

## Contract navigation

- [Documentation catalog](docs/README.md) lists project indexes and domain contracts. Project indexes own canonical paths, stable project IDs and cross-domain invariants.
- [Repository workflow](docs/repository-workflow-contract.md) owns CI, release, allocation-validation records and validation procedures.
- [Environment ownership](docs/repository-environment-contract.md) owns configuration and secret boundaries.
- Read scoped instructions in every affected domain, including cross-domain consumers.

## Git Tracker ownership

- `servers/git-tracker-api`: Rust API, PostgreSQL persistence and background maintenance.
- `apps/git-tracker`: Web application.
- `actions/git-tracker`: Official GitHub Action.
- `protos/gittracker/v1`: Connect schema and generated Go and TypeScript bindings.

Read [Git Tracker contracts](docs/project-git-tracker.md) before changing these paths.
