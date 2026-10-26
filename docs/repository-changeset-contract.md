# Changeset contract

## Ownership and scope

`scripts/changeset/` owns the repository Go CLI in the root Go module. `.changeset/`
stores pending change records. Package identities come from `package.json` names
in the root and the workspaces selected by `pnpm-workspace.yaml`, including private
applications and servers. Rust components without their own npm manifest use the
owning distribution's npm name. Do not invent a second component registry.

This is a repository-specific JSON format, not the upstream Changesets Markdown
format. It records intent and previews notes. It does not bump versions, expand
dependency releases, publish artifacts, edit public documentation, or consume or
delete records. Existing release workflows remain independent.

## Record format

Each `.changeset/<id>.json` has this shape:

```json
{
  "schemaVersion": 1,
  "packages": {
    "@delinoio/lane": "patch"
  },
  "summary": "Internal engineering explanation in Markdown.",
  "public": "Optional user-facing release note in Markdown."
}
```

Use the exact names printed by `pnpm changeset packages`. `packages` is nonempty
and maps each affected package to `patch`, `minor`, or `major`. `summary` is required
and nonblank. `public` is optional; absent or blank means internal-only. Public
text applies to every package in the record; use separate records when audiences
need different notes. Text is authored Markdown. Do not put secrets in either
field: internal-only controls changelog selection, not access to Git history.

Unknown fields, unsupported schema versions, duplicate JSON keys, unknown package
names, invalid bumps, empty summaries, trailing JSON, symlinks and unexpected
files in `.changeset/` fail validation. `README.md` is the only ignored file.
Records use lowercase kebab-case IDs. Package names must be unique across the
workspace inventory. A package rename requires updating pending records.

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

The npm script runs `go run ./scripts/changeset`. Repeat `--package name:bump`
for multiple packages. Omit `--public` for internal-only changes. `add` refuses to
overwrite an existing record. For long Markdown, edit the JSON file after creation.

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
that every diff has a changeset. Run `go test -race ./scripts/changeset` and
`go vet ./scripts/changeset` when changing the CLI.
