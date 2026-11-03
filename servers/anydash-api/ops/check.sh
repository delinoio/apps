#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-ALv2
set -euo pipefail
ops_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
docker run --rm --entrypoint promtool -v "$ops_dir:/etc/anydash-monitoring:ro" prom/prometheus:v3.15.1 check rules /etc/anydash-monitoring/alerts.yml /etc/anydash-monitoring/recording.yml
docker run --rm --entrypoint promtool -v "$ops_dir:/work:ro" -w /work prom/prometheus:v3.15.1 test rules rules.test.yml
docker run --rm --entrypoint amtool -v "$ops_dir:/work:ro" prom/alertmanager:v0.34.1 check-config /work/alertmanager.example.yml
docker run --rm --entrypoint promtool -v "$ops_dir:/etc/anydash-monitoring:ro" prom/prometheus:v3.15.1 check config --syntax-only /etc/anydash-monitoring/prometheus.yml
node --test "$ops_dir/config.test.mjs"
