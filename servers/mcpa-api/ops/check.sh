#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-ALv2
# Offline validation of the Managed CPA monitoring assets. Set PROMTOOL to a
# local promtool binary to run without Docker.
set -euo pipefail
ops_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
run_promtool() {
  if [[ -n "${PROMTOOL:-}" ]]; then
    (cd "$ops_dir" && "$PROMTOOL" "$@")
  else
    docker run --rm --entrypoint promtool -v "$ops_dir:/work:ro" -w /work prom/prometheus:v3.15.1 "$@"
  fi
}
run_promtool check rules thresholds.yml recording.yml alerts.yml
run_promtool test rules rules.test.yml recording.test.yml
run_promtool check config --syntax-only prometheus.yml
node --test "$ops_dir/config.test.mjs"
