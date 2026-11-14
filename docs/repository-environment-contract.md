# Repository Environment Contract

## Configuration ownership

The public repository has no application services, environment selectors, secret provider integration or production credentials. New services must define their configuration schema, validation, local defaults and credential boundaries in their domain contracts.

## Local state

Never commit `.infisical.json`, `.dev-environment/`, real `.env` files or user, production, release and signing credentials. Service-local `.env.example` files may contain names, validation guidance, placeholders and safe loopback defaults only. Do not add a root environment example.

## Process boundaries

Use explicit argument arrays and `shell: false` for child processes. Document any required platform shim. Forward termination signals and wait for owned child processes to exit. Documentation servers must declare a fixed loopback port and fail on conflicts.
