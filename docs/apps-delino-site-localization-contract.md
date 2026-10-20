# Delino localization contract

## Ownership

`delino-site` and `delino-docs` use English (`en`) as their source language and publish these target languages, identified by BCP 47 tags: Korean (`ko`), Japanese (`ja`), Simplified Chinese (`zh-Hans`), Spanish (`es`), French (`fr`), German (`de`) and Brazilian Portuguese (`pt-BR`). Both apps register the same set. Right-to-left languages are not registered until the layouts define mirroring. Each app owns its `i18n/locales.json` language registry and `i18n/messages/<language>.json` flat text catalog. Documentation sources live under `apps/delino-docs/docs/en`; translated pages use sibling language directories. Shared tooling lives in `scripts/localization/`.

English routes keep their existing paths. Other languages use `/<language>/`, for example `/ja/` and `/zh-Hans/`. Language selection is explicit and links between the two apps retain the selected language. Rspress owns documentation language selection, navigation, search and HTML language metadata. The marketing app emits one HTML entry per registered language and loads only its selected text catalog.

## Translation workflow

Run these commands from the repository root or either owning app. At the root, `pnpm i18n:translate` and `pnpm i18n:check` run the corresponding command in every workspace that provides it, currently Delino Site and Delino Docs. Future apps participate by providing these package scripts. Workspaces without localization scripts are skipped. Generation runs sequentially to share ChatGPT authentication without lock conflicts and stops if an app fails. Account commands use the same shared local session from either location.

- `pnpm i18n:login`: continue with ChatGPT in the system browser and authorize ChatGPT plan usage.
- `pnpm i18n:models`: list models available to the connected account.
- `pnpm i18n:translate`: generate missing or source-changed translations. Set `DELINO_TRANSLATION_MODEL` to a model slug from the account catalog; otherwise use the first listed model.
- `pnpm i18n:check`: validate catalog keys, document coverage, protected syntax and source fingerprints without network access or credentials.
- `pnpm i18n:logout`: end the renewable session and clear local credentials while retaining the host and registration identifiers.

`pnpm i18n:translate ko` restricts generation to one target language, including when run for all apps from the root. `pnpm i18n:profiles` lists saved registrations; `pnpm i18n:login <profile-id>` reauthorizes and selects one, and `pnpm i18n:login --new` adds a separate registration. A fresh host UUID is an opaque OAuth installation identifier, not a persisted business entity.

To add a language, add its BCP 47 `lang` and native `label` to each app's registry, run root `pnpm i18n:translate`, review the generated files, and run each app's `pnpm test`. The registry drives routes and language menus; adding a language must not require switch statements in runtime code. Keep English text in catalogs rather than components. Keep product names unchanged in translations.

The generator treats catalog values and Markdown pages as translation units. It batches up to 40 catalog values or one Markdown page per request, and saves completed batches so reruns resume remaining work. It records SHA-256 source fingerprints in each app's `i18n/state.json`. It preserves unchanged human-reviewed translations. Changed units are replaced only by an explicit translation command. New source text, missing output, obsolete units and invalid translations fail the offline check. Remove obsolete catalog keys/pages and their state entries explicitly when removing source content. Review translations for meaning; structural validation cannot establish linguistic accuracy. Source MDX fails validation until an explicit contract defines how to protect its executable content.

Documentation `theme.*` catalog entries originate from the pinned Rspress theme and retain its MIT license. Preserve `apps/delino-docs/i18n/NOTICE` and `RSPRESS-LICENSE` in source and generated distributions. Other repository-owned catalogs and tooling use Apache-2.0.

Code spans, fenced code, placeholders, Markdown link destinations and non-text frontmatter fields must be preserved during translation. Internal absolute document links are localized by the tool after translation. Public content stays limited to the curated documentation app; the generator must not read internal repository contracts as translation input.

## ChatGPT authentication and inference

Translation is a local maintainer operation. The static apps do not authenticate visitors or invoke AI. The CLI follows [Sign in with ChatGPT](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt) and the official [registration](https://developers.openai.com/siwc/token-sharing-open-source/sign-in), [session](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions) and [inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference) contracts. It requires an eligible ChatGPT plan and the granted `chatgpt.tokens.use.direct` permission; identity sign-in alone is insufficient.

The CLI generates a stable installation host ID, uses dynamic client registration, PKCE, random state and nonce, and binds its callback listener to `127.0.0.1`. It verifies ID-token signature, issuer, audience, expiration and nonce with `jose`. Account registrations remain separate, and returning sign-in reuses the issued client ID. Tokens are stored atomically with owner-only permissions outside the checkout under `~/.config/delino-translations/`. Serialize local auth operations to avoid rotating-refresh-token races. Do not read Codex credentials or put tokens in source, browser bundles, logs or CI artifacts.

Discover account-specific models through the public models endpoint. Inference uses the public Responses API with `store: false` and `stream: true`. Accept output only after `response.completed`; fail on refusals, incomplete responses, usage limits or interrupted streams without recording the unit as translated. Show that generation uses the ChatGPT plan and provide `https://chatgpt.com/settings/usage` for managing usage. No API key, copied OAuth token or hosted translation service is required.

## Validation

Each app's `pnpm test` runs offline localization validation, TypeScript checks and a production build. Shared tooling tests use mocked authentication and inference; real sign-in, consent, account model discovery and completed plan-backed inference require separate account acceptance. Local builds do not establish Cloudflare deployment acceptance. Keep source revision, commands, results and unperformed account/platform checks in commits or CI records.
