# Repository Billing Contract

## Scope

This contract defines the currency and monetary precision for every user-facing charge, price, balance and credit in the repository. Project and domain contracts own their pricing, products and payment flows, but they must follow these rules.

## Currency

- All charges to users use USD. Do not price, charge or keep balances in another currency.
- Displaying an approximate amount in another currency is allowed only as an informational label. The charged and stored amount stays in USD.

## Precision

- The smallest monetary unit is 0.000001 USD (one micro-USD). Store and compute all prices, usage costs, credit amounts and balances with this precision.
- Represent amounts as signed 64-bit integers that count micro-USD (1 USD = 1,000,000 micro-USD). Never use binary floating-point types (`float`, `double`, JavaScript `number` arithmetic) for monetary values.
- Name integer amount fields and columns with a `_usd_micros` suffix, for example `balance_usd_micros` or `unit_price_usd_micros`.
- In TypeScript, handle amounts as `bigint`, which is the default protobuf-es mapping for `int64`. In Go, use `int64`. In SQL, use a 64-bit integer column such as `BIGINT`.
- Payment providers can charge at a coarser precision, such as USD cents. Convert at the payment boundary only, and document the conversion and its rounding direction in the owning domain contract.
- Any calculation that can produce a fraction smaller than one micro-USD, such as per-unit usage pricing, must document its rounding direction in the owning domain contract. Do not round implicitly.
- Round only for display. A rounded display value must never be stored or used in further calculations.

## References

- [Repository defaults](repository-defaults.md)
