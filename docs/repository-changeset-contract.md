# Changeset contract

## Ownership and scope

`tools/changeset/` owns the repository Go CLI in the root Go module. `.changeset/`
stores pending change records. Package identities come from `package.json` names
in the root and the workspaces selected by `pnpm-workspace.yaml`, including private
applications and servers. Rust components without their own npm manifest use the
owning distribution's npm name. Do not invent a second component registry.

Records use the [Changesets Markdown format](https://github.com/changesets/changesets):
YAML frontmatter maps package names to release types, followed by a Markdown body.
The Go CLI parses and validates this format itself. It does not depend on the
upstream Changesets parser or CLI. It records intent and previews notes; it does
not bump versions, expand dependency releases, publish artifacts, edit public
documentation, or consume or delete records. The changeset-driven release workflow consumes validated records separately; see
the [release procedure](repository-workflow-contract.md#project-release-preparation).

## Record format

Each `.changeset/<id>.md` has this shape:

```markdown
---
"@delinoio/lane": patch
---

Internal engineering explanation in Markdown.

<!-- public-changelog:start -->

Optional user-facing release note in Markdown.

<!-- public-changelog:end -->
```

Use exact names printed by `pnpm changeset packages`. Frontmatter contains only
package-to-release mappings: `patch`, `minor`, `major`, or `none`. No schema version,
summary field or public metadata field belongs in frontmatter. Package names must
be unique across the workspace inventory. A package rename requires updating
pending records. The reader also accepts standard empty changesets with empty
frontmatter and empty bodies; `add` requires at least one package and a nonblank
internal summary. `none` records no version bump intent.

The public block is a repository convention within the standard Markdown body.
Its two HTML comment markers must occur exactly once, on separate unindented lines,
at the end of the body and outside code fences. Text before the block is the
internal summary. Omit the entire block for internal-only changes. Absent or blank
public text produces no public note. Ordinary unmarked Changesets Markdown is
accepted and treated as internal-only. Public text applies to every package in the
record; use separate records when audiences need different notes. The CLI rejects
reserved markers in `--summary` and `--public` arguments to prevent ambiguous blocks.

The entire Markdown body remains in Git. Public selection is performed by this Go
CLI, not by the Changesets file format itself. Do not publish the unfiltered body
or store secrets in it. Release preparation retains this selection rule in its committed manifest;
deployment and changelog publication remain outside this Go tool's scope.

Invalid YAML, duplicate package keys, unknown package names, invalid release types,
malformed public blocks, symlinks and unexpected files fail validation. `README.md`
(case-insensitive) and `config.json` are ignored; the latter is reserved for upstream
configuration and is not interpreted by this tool. Record IDs use lowercase
kebab-case; `readme` is reserved. The former repository-specific JSON records are not accepted.

## Public note authoring

Follow the public changelog audience rules in [root instructions](../AGENTS.md#changeset-procedure).
The internal summary explains the engineering change. The public block explains
what changed for the affected product's end users. Package names identify ownership;
they do not determine the audience of the prose.

Examples of the distinction:

- Internal: "Reduce redundant search requests." Public: "Search results load faster."
  Use the public claim only when the change and validation support it.
- Internal: "Add changeset validation to CI." Public: omit the block. This changes
  the development process and does not itself add a user-facing product feature.
- Requested initial-release example: "Welcome to Delino! This is our first release."
  Do not replace the release announcement with instructions for the changeset CLI.

A supported API or developer tool can have developers as its end users. In that
case, describe its public behavior or required migration steps. Repository-local
maintenance commands and implementation details still belong in the internal
summary. Do not invent benefits to fill an optional public block.

Run `pnpm changeset changelog` and read the selected text as an end user before
committing. Check that it explains an actual user-visible change or a requested
release announcement, without requiring knowledge of the repository's internals.
The CLI validates structure and selection; audience and factual accuracy require
this authoring review.

## Commands

Run from the repository root with the Go version selected by `go.mod`:

```sh
pnpm changeset packages
pnpm changeset add --id improve-search --package '@delinoio/lane:patch' --summary 'Reduce redundant search requests.' --public 'Search results load faster.'
pnpm changeset check
pnpm changeset status
pnpm changeset changelog
pnpm changeset changelog --internal
```

The npm script runs `go run ./tools/changeset`. Repeat `--package name:bump`
for multiple packages. Omit `--public` for internal-only changes. `add` refuses to
overwrite an existing record. For long Markdown, edit the Markdown file after creation.

`status` emits all pending records as JSON, including internal summaries.
`changelog` writes an unreleased Markdown preview to stdout, grouped by sorted
package name and sorted record filename. The default output includes only explicit
public text; it never substitutes internal summaries. `--internal` explicitly
selects internal summaries instead. Neither mode writes files. An empty selection
produces no output. Version bumps remain intent, not calculated release versions.

## Contributor procedure and validation

Add a record for component behavior changes, with all affected owning npm names.
Documentation-only and workflow-only changes do not require a record. Run
`pnpm changeset check` before committing. CI checks all pending records in its
existing unconditional formatting job so package renames cannot leave stale
records. That job also runs the CLI's Go tests and vet check. CI does not enforce
that every diff has a changeset. Run `go test -race ./tools/changeset` and
`go vet ./tools/changeset` when changing the CLI.
