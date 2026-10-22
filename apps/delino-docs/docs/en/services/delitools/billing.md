# DeliTools billing

DeliTools bills your tenant for the AI tokens that your requests use. Delcore rates the usage and keeps your balance, credits and invoices. This page explains how usage becomes a charge.

## What is measured

- DeliTools reads the totals of **input tokens** and **output tokens** that the AI provider reports for each attempt. It uses these totals as they are reported.
- The reported totals already include any cached or reasoning tokens. DeliTools does not add those parts again, gives no separate cache discount and adds no surcharge for reasoning.
- DeliTools reports input and output usage to Delcore separately, and Delcore rates them with their own prices.

## Prices

- The service operator sets a price per token direction (input and output) for each model in Delcore. Different models can have different prices.
- Translation and extraction share the same prices. Extraction always uses `openai/gpt-6-luna`, so its charge uses the input and output prices of that model.
- A model without both prices registered cannot run. The request fails with `ERROR_REASON_MODEL_UNAVAILABLE` and no other model is used instead.
- A price change applies to executions that start after the change. Usage that already started keeps the price at its start.
- Your plan decides included usage, your balance and whether you can continue after the balance is used up. See your plan and invoices in Delcore.

## What is charged

- **Every attempt with known usage is charged.** This includes an attempt whose result is invalid, whose extraction fails, whose answer arrived before you canceled, or whose result never reaches you.
- **Retries are separate attempts.** A transient provider failure (a network error or an HTTP 5xx or 429 reply) returns no usage, so DeliTools retries it without a charge for the failed try. A completed provider reply with known usage ends the attempt loop and is charged, even when its output is invalid, because DeliTools never retries to repair invalid output. A reply without usage costs nothing.
- **Unknown usage is not charged.** If the provider does not report usable token totals, DeliTools does not estimate a charge and does not look the usage up later.
- **Aborted calls are not charged.** If you cancel, or the execution timeout elapses, while the provider call is still running, no usage report arrives and nothing is charged, even if the provider had already started work. This is a known limit: DeliTools does not look this usage up afterwards.
- **Usage must be recorded.** DeliTools stores the usage of every attempt before it answers. If it cannot store it, the request fails with `ERROR_REASON_DEPENDENCY_UNAVAILABLE` and you receive no result.
- **Rejected requests are not charged.** Requests that fail validation, authentication or authorization, or that hit a limit or an unsupported schema, never reach the AI provider.
- **Every call is new.** DeliTools has no idempotency key and no result replay. If you send the same request again, it runs again and is charged again.

## When charges appear

DeliTools saves the usage of each attempt before it answers you, and then reports it to Delcore. Usage reporting is separate from your request, so a charge can appear in Delcore shortly after the call. If Delcore is unavailable, DeliTools keeps trying with the same identifiers, so retries do not create duplicate charges.

Usage records that DeliTools cannot deliver are deleted 30 days after they were created, and retries do not extend that period. Usage that was never delivered in that time is not charged. DeliTools does not estimate it.

You can read your usage and charges from Delcore. DeliTools responses do not include them.

## Balance checks and overspend

Delcore checks your authorization and billing status before each request, and again before each retry. It does not reserve money or tokens for the call. The cost is known only after the call finishes. Several requests that run at the same time can therefore use more than your remaining balance or included usage. When the balance or a limit is exhausted, Delcore blocks later requests with `ERROR_REASON_BILLING_BLOCKED`.

If Delcore cannot be reached, DeliTools refuses new paid requests instead of running them.

## Blocked requests

`ERROR_REASON_BILLING_BLOCKED` means Delcore blocked paid use for your tenant. Common causes are an exhausted balance, a failed renewal or an expired paid period, an open dispute, an operator block, or a tenant that is being deleted. Resolve the cause in Delcore. For example, top up your balance or renew your subscription. Tenant management and payment recovery stay available while paid use is blocked.
