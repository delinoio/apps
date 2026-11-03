---
"@servers/mcpa-api": patch
---

Refresh Gemini CLI OAuth tokens in the Managed CPA data plane before the SDK executor runs. An expired token is refreshed once per credential with the OAuth client stored in the credential payload or the operator-configured client, through the destination-guarded provider transport and outside call metering. The new payload is stored encrypted and other replicas are notified. A rejected refresh token returns 401 with the provider error code and a provider authentication record; other refresh failures return 502.
