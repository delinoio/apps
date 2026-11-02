# AnyDash load test

`anydash-load.mjs` is the capacity gate from the [server contract](../../../docs/servers-anydash-api-contract.md#build-and-test): 1,000 concurrent active users and 5,000 collector devices with 1-minute heartbeats and local usage uploads and 5-minute external polling snapshots for 30 minutes, without errors, against a production-like deployment. The maintainer records the result in the release validation record. Mocked and load tests do not replace real-account, real-device or store acceptance.

## Provision

Prepare `users.json` outside the repository: an array of 1,000 objects with `accessToken` (Logto token for `https://anydash.api.delino.io`) and `delcoreAccessToken` (Logto token for the Delcore resource) of dedicated test accounts. Then run:

```sh
node servers/anydash-api/loadtest/anydash-load.mjs provision --api https://<target> --users users.json --out state.json --devices-per-user 5
```

Provisioning connects each account, enrolls five collector devices per user (5,000 devices), creates a Codex and a Vercel connection registered on every device of the user and a personal dashboard with a usage series widget. The unauthenticated enrollment limit of 10 starts per minute per client address paces enrollment at one device every 6.1 seconds, so provisioning 5,000 devices from one address takes about 8.5 hours. The command exits non-zero when any request fails or any account, device, connection registration or dashboard widget is missing. It still writes the state file, marked incomplete, because the file holds the credentials of the enrolled devices; `run` refuses an incomplete state, so provision again with a fresh set of test accounts. The state file holds device credentials with mode 0600; keep it out of the repository and delete it after the validation.

## Run

```sh
node servers/anydash-api/loadtest/anydash-load.mjs run --api https://<target> --state state.json --minutes 30
```

The default `--mode gate` is the capacity gate. It refuses to start, without sending requests, unless provisioning completed for at least 1,000 users and 5,000 devices and `--minutes` is at least 30. For a smaller trial, run `--mode smoke` with any completed population and duration; its result reports `"capacityGate": false` and is not a capacity gate result.

Every user queries its dashboard data and account every 30 to 60 seconds. Every device sends a heartbeat and an upload every minute, acquires leases recommended by the heartbeat, reports a snapshot every 5 minutes for connections whose lease it holds. Provisioning takes hours and device access tokens are valid for one hour, so each device refreshes its credentials before its first request and then 10 minutes before the returned access token expiry. Refresh rotates the refresh token, so the command saves the state file every minute and at the end; always run with the latest state file, because presenting a rotated refresh token revokes the device. Rejected snapshots after a lease change (`failed_precondition`) are expected and not counted as errors. The command prints progress every minute and a JSON result with the mode, per-method request and error counts, and exits non-zero when any request failed. Record the source revision, deployment, command and result; never record tokens.
