# apps/delino-site working instructions

- Follow root and parent instructions.
- This directory owns the Delino web application.
- Read the [project index](../../docs/project-delino-site.md) and [web contract](../../docs/apps-delino-site-web-contract.md).
- Keep frontend development on fixed loopback port 46314 and fail on conflicts.
- Run package-local `pnpm test` for frontend changes. Verify deployment separately.

## Translation maintenance

- When adding or editing visitor-facing text, agents must update the English and Korean messages in the same change without a separate translation request. Treat the language edited for the task as the source for that change.
- Keep matching message keys and interpolation values in both languages. Include visible labels, accessible names and user-facing metadata. Preserve meaning, qualifications, product names, code and identifiers; keep documentation links in the selected language.
- Before finishing, compare the changed messages in both languages for omissions and report untranslated content or checks that were not performed. Ask when the source meaning is ambiguous.
