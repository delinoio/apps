---
"@servers/mcpa-api": patch
---

Apply the resource and tenant budget gate after the model allowlist, as the call path contract orders them. The authorizer interface splits the budget check into `CheckBudgets`, which the proxy engine calls after the allowlist and before credential selection; `AuthorizeCall` keeps the tenant and paid-invocation gates before the body is read. A request for a disallowed model now returns `model_not_allowed` instead of `budget_exhausted` or `dependency_unavailable` and no longer increments `mcpa_budget_rejections_total`. Allowed models, rejection codes, messages and metric labels are unchanged.

<!-- public-changelog:start -->

A call for a model that the resource's allowlist does not permit now always returns `model_not_allowed`, even when the resource or tenant budget is exhausted or its billing period cannot be read.

<!-- public-changelog:end -->
