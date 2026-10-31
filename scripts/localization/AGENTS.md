# Localization tooling instructions

- Follow root instructions and the [Delino localization contract](../../docs/apps-delino-site-localization-contract.md).
- This directory owns shared local translation generation and offline validation for Delino Site and Delino Docs.
- Keep authentication in the local CLI. Do not import authentication modules into either frontend.
- Test authentication and generation with fixtures; record real ChatGPT account acceptance separately.

- Keep translated coverage as the default. Use exact source-only declarations and registered section markers only for English-only requirements in owning product contracts; follow the localization contract. Do not bypass discovery, freshness checks or protected syntax validation.

- `delino-console` consumes this tooling under the shared Delino localization contract and [its web contract](../../docs/apps-delino-console-web-contract.md).
