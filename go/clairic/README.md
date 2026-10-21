# Clairic Go client

A standalone Go client for the [Cloudflare Workers AI Clairic model](https://developers.cloudflare.com/workers-ai/models/clairic/). Supports extraction, estimation, forecasting and embedded images. Requires Go 1.27.3 or later. Uses only the Go standard library.

```sh
go get github.com/delinoio/apps/go/clairic
```

```go
client, err := clairic.NewClient(accountID, apiToken, nil)
if err != nil {
    return err
}

response, err := client.Run(ctx, clairic.Request{
    State: "Invoice INV-2291: 4 pallets at $250 each, shipping $80.",
    Fields: map[string]clairic.Field{
        "invoice_number": {Type: clairic.TypeText},
        "total": {
            Type: clairic.TypeNumber,
            Unit: "USD",
            Instructions: "Amount due including shipping",
        },
        "days_to_pay": {
            Type: clairic.TypeEstimate, Min: 0, Max: 120, Step: 1,
            Instructions: "Days until this invoice is paid",
        },
    },
})
if err != nil {
    return err
}

var total *float64
if err := response.Answers["total"].DecodeValue(&total); err != nil {
    return err
}
// A nil total means the model returned null.
```

Import `github.com/delinoio/apps/go/clairic`. Supply a Cloudflare account ID and an API token with Workers AI permissions. The example uses caller-provided `ctx`, `accountID` and `apiToken` variables.

Fields support `TypeText`, `TypeNumber`, `TypeDate`, `TypeTime`, `TypeDatetime`, `TypeDuration`, `TypeList`, `TypeEstimate` and `TypeForecast`. For lists, set `Items` to `ItemText`, `ItemNumber` or `ItemDate`. Forecasts support `Horizon` and `Interval`. Set pointer options such as `Decimals` when explicit zero values must be sent. Numeric/date bounds and steps accept numbers or strings as specified by Cloudflare. Field-specific semantic validation is performed by the service.

Use `Request.Images` with `Image{DataURL: "data:image/png;base64,..."}` or `Image{ContentType: "image/jpeg", Base64: encoded}`. PNG, JPEG and WebP are supported, with at most four images, 4 MiB per decoded image and 8 MiB total. Remote image URLs are not supported. The request body is limited to 16 MiB.

Answers preserve JSON values, guesses, estimate statistics, quantiles and forecast steps as `json.RawMessage`. This retains nulls and exact JSON numbers. Use `DecodeValue` with your application type. Confidence, null probability, spans, truncation, optional state text and token usage are available on the response. Enable extra output with `Options{IncludeQuantiles: true, ReturnStateText: true}`.

The default HTTP timeout is two minutes. Use context deadlines or supply `Config.HTTPClient` to control it. `Config.BaseURL` can target a trusted proxy and must include its API prefix. This URL receives the bearer token. The client performs no automatic retries and emits no logs.

Use `errors.As` to inspect `*clairic.APIError` for HTTP failures or Cloudflare API failures. It contains `StatusCode`, `Errors`, `Messages` and `RequestID` (the `CF-Ray` header). Raw HTTP bodies and API messages are excluded from the error string. Use `errors.Is` to inspect transport and context errors.

Licensed under Apache-2.0. This client is not an official Cloudflare SDK. Clairic is a preview model; its service contract may change.
