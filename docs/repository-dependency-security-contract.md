# Dependency security contract

Keep root dependency locks, Go 1.27.3 and the immutable Tauri/CEF revision. A fixed runtime pin or unavailable compatible patch is not a vulnerability fix. Preserve raw audit failures and unresolved findings; do not add blanket exclusions.

The shared Go module uses `golang.org/x/net` 0.60.0 or later for the HTTP/2 security fixes. The Rust workspace lock uses `quinn-proto` 0.11.18 or later and `rustls` 0.23.46 or later for the QUIC and TLS security fixes. Keep their required transitive dependencies consistent when refreshing either lock.

Inherited native constraints include GTK3 `glib` 0.18.5 (no compatible patched release), Tauri selectors `rand` 0.7.3, Tauri CLI RPM/PGP `ml-dsa` 0.0.4, pinned xcb `quick-xml` 0.30.0, and macOS/RPM signing `rsa` 0.9.10. Their presence and reachability must be checked against this repository and its pinned prebuilt tools; migration does not establish remediation. Retain unmaintained/yanked notices. Use `pnpm audit`, `cargo audit` and `govulncheck -show verbose ./...`; record results and unresolved upstream constraints in CI or PR records.
