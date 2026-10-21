# Clairic Go Client Contract

## Scope

`go/clairic` owns the independent `clairic` client package. It calls the hosted model; it does not run inference locally or provide a server.

## Runtime and Language

Go 1.27.3, matching the repository Go baseline. The package is a separate standard-library-only module.

## Users and Operators

Go applications supply their own Cloudflare account ID and Workers AI API token.

## Interfaces and Contracts

`NewClient` accepts credentials and optional HTTP configuration. `Run` accepts a context and typed request, then posts JSON to `/client/v4/accounts/{account_id}/ai/run/@cf/cloudflare/clairic`. The request model defaults to `clairic`.

Field types cover text, number, date, time, datetime, duration, list, estimate and forecast. Enum types represent list items, scale and forecast interval. Mixed numeric/date bounds and steps use `any`; the upstream service validates field-specific semantics. Pointer fields preserve explicit zero and false values. Images accept data URLs or base64 objects. Requests require state or images and at least one field. The client enforces the documented image count and decoded byte limits and the 16 MiB request limit.

Responses decode the Cloudflare REST envelope. Answers retain mixed values, guesses, estimate statistics, quantiles and forecast steps as `json.RawMessage` to preserve JSON types, nulls and numeric precision. `Answer.DecodeValue` decodes a value into caller-owned Go storage. Confidence and probability pointers distinguish absent values from zero. Usage, truncation and optional state text remain available.

HTTP failures and `success: false` return an `APIError` with status, Cloudflare errors, messages and request ID. Malformed successful envelopes return decoding errors. Context cancellation and transport errors remain inspectable with `errors.Is`. No automatic retries are performed.

## Storage

No persistence or cache. Images are embedded in model requests as required by Clairic; this is an inference input, not an object-storage upload workflow.

## Security

Credentials are explicit constructor arguments. Never log credentials, request content or model answers. Custom base URLs are trusted by the caller and receive the bearer token. The default HTTP client has a two-minute timeout. Callers may supply an HTTP client and context deadlines.

## Logging

The library emits no unsolicited logs. Applications may instrument their HTTP transport. Errors expose status and request ID for structured application logging without requiring raw payload logging.

## Build and Test

Run `go test -race ./...` and `go vet ./...` from `go/clairic`. Root `go test ./...` does not traverse this nested module. HTTP fixtures verify wire encoding, response types, errors and cancellation without credentials. Live Cloudflare acceptance is separate and requires explicit account access. CI coverage for this independent module is not configured by this contract.

## Dependencies and Integrations

Only the Go standard library and the Cloudflare Workers AI REST API. The root module and server dependency graph remain separate.

## Change Triggers

Update the project index, this contract and public examples when module ownership or API behavior changes. Update `go/AGENTS.md` for development-procedure or ownership changes.

## References

- [Project index](project-clairic.md)
- [Repository defaults](repository-defaults.md)
- [Clairic documentation](https://developers.cloudflare.com/workers-ai/models/clairic/)
- [Input schema](https://developers.cloudflare.com/workers-ai/models/clairic/schema-input.json)
- [Output schema](https://developers.cloudflare.com/workers-ai/models/clairic/schema-output.json)
