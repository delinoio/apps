# CF Clairic client contract

## Scope

`crates/cf-clairic` owns the `cf-clairic` library and its public Rust API.

## Runtime and Language

Rust 2024, asynchronous Tokio-compatible HTTP through reqwest with Rustls. The package is an explicit root Cargo workspace member and must also build independently from its package archive.

## Users and Operators

Rust applications supply a Cloudflare account ID and Workers AI API token. Maintainers prepare the crate for crates.io; publication requires a separate explicit release request.

## Interfaces and Contracts

`Client::run` posts JSON to `/client/v4/accounts/{account}/ai/run/@cf/cloudflare/clairic`. Requests always select `clairic`. Typed enums cover field types, list items, scales, forecast intervals and image formats. Optional schema properties are omitted when unset. Responses retain null values, uncertainty metadata, usage and unknown answer properties. Values and forecast steps whose shapes are not constrained by the upstream output schema remain JSON values. Span offsets are upstream character offsets, not Rust byte offsets.

The client decodes the Cloudflare REST envelope and distinguishes transport, HTTP/API, malformed response and invalid configuration errors. It does not retry billed inference automatically. Default timeout is 120 seconds and redirects are disabled. Callers can configure timeout and explicitly select a trusted HTTP(S) API base for testing or a proxy. Provider-side field, image and request limits remain authoritative; the client checks nonempty fields and state-or-images before sending.

## Storage

No persistence or cache. Images are embedded data URLs or base64 objects; there is no file upload service or R2 integration.

## Security

Tokens are supplied at runtime and marked sensitive in HTTP headers. Client debug output and structured logs exclude credentials, account IDs, inputs and answers. Error display excludes upstream response bodies; structured API errors remain available to callers and may contain sensitive content. Only use trusted custom API bases.

## Logging

Use tracing events for completion status. Do not install a global subscriber or log request/response content.

## Build and Test

Run root `cargo test`, plus `cargo test -p cf-clairic`, `cargo fmt --check -p cf-clairic` and `cargo package -p cf-clairic`. Package verification must compile the extracted archive independently of workspace patches. Offline HTTP tests use disposable loopback servers, not credentials. Actual Cloudflare acceptance is a separate check. Include LICENSE and NOTICE in the archive. A future release checks package name availability, version, metadata and credentials before an explicitly authorized `cargo publish`; no publication automation is added here.

## Dependencies and Integrations

Use registry dependencies only: reqwest, serde, serde_json, thiserror and tracing. Tokio is a test dependency. No native shell dependencies or workspace path dependencies are distributed.

## Change Triggers

Keep the [project index](project-cf-clairic.md), this contract and public crate documentation aligned with API and packaging changes. Ownership or procedure changes also update the owning AGENTS.md.

## References

- [Repository defaults](repository-defaults.md)
- [Model reference](https://developers.cloudflare.com/workers-ai/models/clairic/)
- [Input schema](https://developers.cloudflare.com/workers-ai/models/clairic/schema-input.json)
- [Output schema](https://developers.cloudflare.com/workers-ai/models/clairic/schema-output.json)
