# Repository Authentication Contract

## Scope

This contract defines user authentication and identity for every project and domain in the repository. Project and domain contracts may add authorization rules, but they must not introduce a separate identity system.

## Identity provider

- [Logto](https://docs.logto.io/) is the only identity provider. All services share one Logto tenant per deployment environment, so a user signs in once and uses the same account across every service.
- Do not add service-local password storage, sign-up flows, session stores that replace Logto sessions, or another identity provider. Add social, enterprise or other sign-in methods as Logto connectors in the shared tenant.
- Register each user-facing app as a Logto application and each backend API as a Logto API resource in the shared tenant. Record the application type, API resource indicator and required scopes in the owning domain contract.

## User identity

- The Logto user ID (the `sub` claim) is the canonical user identifier across all services. Store it as the foreign reference for user-owned data; do not derive user identity from email addresses, usernames or display names.
- Services may keep local profile or preference records keyed by the Logto user ID. Logto remains the source of truth for credentials and sign-in identities.

## Token validation

Backend services must validate every Logto access token before they trust it. Follow [Logto's access token validation guide](https://docs.logto.io/authorization/validate-access-tokens).

- Read the issuer and JWKS URI from the tenant's OpenID Connect discovery document (`<tenant endpoint>/oidc/.well-known/openid-configuration`), or configure the issuer `<tenant endpoint>/oidc` and JWKS URI `<tenant endpoint>/oidc/jwks`.
- Verify the signature with the tenant's JWKS keys, and verify the issuer and expiration.
- Verify that the `aud` claim matches the service's own API resource indicator. For organization permissions without an API resource, `aud` is `urn:logto:organization:<id>`.
- Split the space-separated `scope` claim and check the scopes the operation requires.
- For organization-level API resources, check that the `organization_id` claim matches the organization in the request.
- Do not accept ID tokens as API credentials.

## Configuration and secrets

- The Logto tenant endpoint, application IDs and API resource indicators are configuration. Application secrets and Management API credentials are secrets and follow the [environment contract](repository-environment-contract.md).
- Service-local `.env.example` files may list the names of Logto settings with placeholders only.

## Logging

Log authentication results with structured fields such as the Logto user ID, the API resource and the rejection reason. Never log access tokens, refresh tokens, ID tokens, authorization codes or application secrets.

## References

- [Logto documentation](https://docs.logto.io/)
- [Validate access tokens](https://docs.logto.io/authorization/validate-access-tokens)
- [Environment contract](repository-environment-contract.md)
