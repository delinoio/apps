# Repository Environment Contract

## Configuration ownership

The public repository has no environment selectors, secret provider integration or production credentials. Each service defines its configuration schema, validation, local defaults and credential boundaries in its domain contract. The [`delino-accounts-api` contract](servers-delino-accounts-api-foundation.md) owns the API server configuration, and the [`delino-accounts` web app contract](apps-delino-accounts-foundation.md) owns the web app build configuration.

## Local state

Never commit `.infisical.json`, `.dev-environment/`, real `.env` files or user, production, release and signing credentials. Service-local `.env.example` files may contain names, validation guidance, placeholders and safe loopback defaults only. Do not add a root environment example.

## Process boundaries

Use explicit argument arrays and `shell: false` for child processes. Document any required platform shim. Forward termination signals and wait for owned child processes to exit. Documentation servers must declare a fixed loopback port and fail on conflicts. Application development servers follow the same rule: `delino-accounts` uses `127.0.0.1:3100` and `delino-accounts-api` uses `127.0.0.1:8100` by default.
