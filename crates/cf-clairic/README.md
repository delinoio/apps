# cf-clairic

Async Rust client for [Cloudflare Workers AI Clairic](https://developers.cloudflare.com/workers-ai/models/clairic/). This independent client is not an official Cloudflare SDK. Clairic is a preview API.

```no_run
use cf_clairic::{Client, Field, FieldType, Request};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::new(
    &std::env::var("CLOUDFLARE_ACCOUNT_ID")?,
    &std::env::var("CLOUDFLARE_AUTH_TOKEN")?,
)?;
let request = Request {
    state: "The invoice total is 1080 USD.".into(),
    fields: [("total".into(), Field::new(FieldType::Number))].into(),
    ..Request::default()
};
let result = client.run(&request).await?;
assert_eq!(result.answers["total"].value, 1080);
# Ok(())
# }
```

Run within a Tokio runtime. Supply a Cloudflare API token with Workers AI access to the selected account. The client never reads environment variables itself.

All nine field types are supported: text, number, date, time, datetime, duration, list, estimate and forecast. Set optional `Field` properties for constraints. Lists require `items`; estimates and forecasts require `min` and `max`. Cloudflare validates field-specific constraints, numeric ranges, field names and image limits. Images can use `Image::DataUrl` or `Image::Base64` with PNG, JPEG or WebP media types. Remote image URLs are not supported. `Options` controls quantiles and returned state text.

Answers retain JSON values, including null, and expose uncertainty information through `Answer::metadata` (for example `confidence`, `p_null`, `span`, `quantiles` and forecast `steps`). These shapes are intentionally flexible because the upstream output schema leaves them unconstrained. Span offsets count characters, not UTF-8 bytes. Model confidence does not guarantee correctness.

`Client` is cloneable and reuses connections. It defaults to a 120-second timeout, disables redirects and does not retry billed requests. `Client::with_options` sets a timeout and trusted API base. The base includes the API prefix, such as `https://api.cloudflare.com/client/v4/`. Custom endpoints receive the bearer token.

Errors distinguish configuration, invalid input, transport, HTTP/API failures and malformed responses. HTTP/API errors include status and structured provider errors when available. Error display and client debug output omit credentials and response content. Provider error fields may contain sensitive content; handle them accordingly. Tracing reports HTTP status only and leaves subscriber configuration to the application.

## Distribution

Licensed under Apache-2.0; LICENSE and NOTICE are included. The package uses registry dependencies and supports independent Cargo packaging. It is prepared for a future crates.io release; no release is implied by this source change. A maintainer must check the version and crate-name availability and explicitly authorize publication. Package compilation and mock tests do not establish live Cloudflare account acceptance.
