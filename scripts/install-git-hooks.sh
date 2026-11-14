#!/usr/bin/env sh
set -eu

canonicalize_existing_directory() {
	(
		CDPATH=
		cd "$1" 2>/dev/null
		pwd -P
	)
}

is_inside_work_tree="$(git rev-parse --is-inside-work-tree 2>/dev/null || true)"
if [ "$is_inside_work_tree" != "true" ]; then
	printf '%s\n' "Git metadata is unavailable; skipping Lefthook installation."
	exit 0
fi

repo_root="$(git rev-parse --show-toplevel)"

# An archive extracted below an unrelated work tree is inside that work tree
# without owning Git metadata; installing there would modify the other repository.
project_root="$(canonicalize_existing_directory "$(dirname -- "$0")/..")"
if [ "$(canonicalize_existing_directory "$repo_root")" != "$project_root" ]; then
	printf '%s\n' "Git metadata is unavailable; skipping Lefthook installation."
	exit 0
fi

git_common_dir="$(git rev-parse --path-format=absolute --git-common-dir)"
configured_hooks_path="$(git config --path --get core.hooksPath || true)"
shared_hooks_path="$git_common_dir/hooks"

if [ -z "$configured_hooks_path" ]; then
	exec lefthook install
fi

case "$configured_hooks_path" in
	/*|[A-Za-z]:/*) absolute_hooks_path="$configured_hooks_path" ;;
	*) absolute_hooks_path="$repo_root/$configured_hooks_path" ;;
esac

canonical_hooks_path="$absolute_hooks_path"
if normalized_hooks_path="$(canonicalize_existing_directory "$absolute_hooks_path")"; then
	canonical_hooks_path="$normalized_hooks_path"
fi

canonical_shared_hooks_path="$shared_hooks_path"
if normalized_shared_hooks_path="$(canonicalize_existing_directory "$shared_hooks_path")"; then
	canonical_shared_hooks_path="$normalized_shared_hooks_path"
fi

if [ "$canonical_hooks_path" = "$canonical_shared_hooks_path" ]; then
	# Lefthook rejects an existing core.hooksPath in linked worktrees even when it
	# already targets Git's shared hooks directory. Force is safe only when both
	# paths resolve to that repository-owned directory; remove this branch when
	# Lefthook handles it.
	exec lefthook install --force
fi

# Preserve Lefthook's protective failure for unrelated custom hook paths.
exec lefthook install
