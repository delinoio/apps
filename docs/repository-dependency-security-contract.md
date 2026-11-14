# Dependency Security Contract

Root manifests and the lockfile own shared dependency resolution. Future projects must document runtime, feature, platform and licensing boundaries.

Run fresh audits for the actual dependency graph after dependency changes. Record the source revision, commands, results, missing tools and unresolved advisories in pull requests, issues or CI logs. Do not treat historical audit results as current acceptance.

Keep unavailable patches, upstream constraints and remaining advisories visible. Do not add blanket exclusions or describe a runtime pin as a vulnerability fix. Preserve upstream licenses and notices.
