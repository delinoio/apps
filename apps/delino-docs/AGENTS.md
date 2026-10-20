# apps/delino-docs working instructions

- Follow root and parent instructions.
- This directory owns the public Delino documentation app.
- Read the [project index](../../docs/project-delino-docs.md) and [web contract](../../docs/apps-delino-docs-web-contract.md).
- Keep public content in this app's `docs/`; repository-root `docs/` owns internal contracts.
- Keep frontend development on fixed loopback port 46315 and fail on conflicts.
- Run package-local `pnpm test` for frontend changes. Verify deployment separately.

## Translation maintenance

- When adding or editing public documentation, agents must update both English and Korean in the same change without a separate translation request. Treat the language edited for the task as the source for that change; English is the default route language, not a requirement to author in English first.
- Pair pages by the same relative path under `docs/en/` and `docs/ko/`. Keep corresponding headings, frontmatter, navigation, sidebar labels and user-facing metadata aligned. When an authorized change moves or deletes a page, apply it to both languages and update their links.
- Preserve the source meaning, requirements, qualifications and examples. Preserve code, commands, API names, identifiers, product names and external URLs; adapt internal page links and heading anchors to the target language. Do not omit or summarize content during translation. Ask when the source meaning is ambiguous.
- Before finishing, compare the changed page pairs for missing content and verify their links and navigation. A successful build does not establish translation accuracy. Report untranslated content or checks that were not performed.
- This procedure applies to curated public content in this app. Do not translate or publish repository-root internal contracts into the public site automatically.
