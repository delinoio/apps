# Clairic Go Client

## Goal

Provide an independent Go client for Cloudflare Workers AI Clairic extraction, estimation and forecasting.

## Project ID

`clairic`

## Domain Ownership Map

| Domain   | Path         | Ownership                                                  |
| -------- | ------------ | ---------------------------------------------------------- |
| packages | `go/clairic` | Independent Go module, public API, examples and HTTP tests |

## Domain Contract Documents

- [Go client contract](packages-clairic-client-contract.md)

## Cross-Domain Invariants

The module path is `github.com/delinoio/apps/go/clairic`. It uses only the Go standard library and Apache-2.0 licensing. It has its own `go.mod` so consumers do not inherit server dependencies. It does not consume repository protocols or require a Go workspace, local replacement, database or deployed service.

## Change Policy

Keep the client contract and public usage examples consistent with API changes. Update ownership instructions when directory ownership or development procedures change. Record validation results and live-account limits in PR or CI evidence.
