#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-ALv2
set -euo pipefail
ops_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
docker run --rm --entrypoint promtool -v "$ops_dir:/etc/lane-monitoring:ro" prom/prometheus:v3.15.1 check rules /etc/lane-monitoring/alerts.yml /etc/lane-monitoring/recording.yml
docker run --rm --entrypoint promtool -v "$ops_dir:/work:ro" -w /work prom/prometheus:v3.15.1 test rules rules.test.yml recording.test.yml
docker run --rm --entrypoint amtool -v "$ops_dir:/work:ro" prom/alertmanager:v0.34.1 check-config /work/alertmanager.example.yml
docker run --rm --entrypoint promtool -v "$ops_dir:/etc/lane-monitoring:ro" prom/prometheus:v3.15.1 check config --syntax-only /etc/lane-monitoring/prometheus.yml
docker run --rm -v "$ops_dir:/work:ro" prom/blackbox-exporter:v0.29.1 --config.file=/work/blackbox.yml --config.check
node --test "$ops_dir/config.test.mjs"
