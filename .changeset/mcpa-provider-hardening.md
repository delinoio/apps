---
"@servers/mcpa-api": patch
---

Harden the provider call path. The upstream relay keeps accepting after transient accept errors with an exponential backoff from 5 ms to 1 s, and Close interrupts the backoff. Each Claude call gets its own relay identity (the call ID as the SOCKS username), tunnels are tracked per call and closed after the final checkpoint, and a five minute idle deadline is the backstop; the idle timer is now set before a tunnel becomes visible to call cleanup. Dial failures for non-public addresses and DNS lookups return fixed error texts and log the original at warn level with the call ID. The Kimi `X-Msh-Device-Name`, `-Model` and `-Id` request headers carry mcpa identifiers instead of the server host name, platform and device ID. `Options.MaxProviderResponseBytes` (default 128 MiB) bounds provider bodies other than the event stream of a streaming call, whatever Content-Type the provider sends, and the provider-to-client bytes of non-streaming Claude relay tunnels; a call over the bound fails with the `provider_response_too_large` error class and a 502 `provider_error` response in the client's format.

<!-- public-changelog:start -->

A provider response larger than 128 MiB, other than the event stream of a streaming request, now fails with a 502 `provider_error` response instead of being read in full. Provider connection errors no longer reveal the server's internal addresses, and Kimi requests no longer send the server host's device details.

<!-- public-changelog:end -->
