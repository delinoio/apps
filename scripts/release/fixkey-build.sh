#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail

log_file="$(mktemp)"
trap 'rm -f "$log_file"' EXIT

for attempt in 1 2 3; do
  set +e
  "$@" 2>&1 | tee "$log_file"
  statuses=("${PIPESTATUS[@]}")
  set -e
  status="${statuses[0]}"
  if [ "${statuses[1]}" -ne 0 ]; then
    exit "${statuses[1]}"
  fi
  if [ "$status" -eq 0 ]; then
    exit 0
  fi

  # hdiutil can fail before mounting its temporary image on macOS runners.
  # Retry only this DMG creation error, keeping all signing checks in Tauri.
  # Remove this workaround when upstream reliably handles image-creation retries.
  if [ "${RUNNER_OS:-}" != macOS ] || [ "$attempt" -eq 3 ] ||
    ! grep -Eq 'error running bundle_dmg\.sh:.*hdiutil: create failed - Resource busy' "$log_file"; then
    exit "$status"
  fi

  # A failed create can leave an unmounted image beside the app. Do not let
  # that partial image enter the next build or the final artifact inventory.
  for image in "target/aarch64-apple-darwin/release/bundle/macos/"rw.*.FixKey_*.dmg; do
    if [ -f "$image" ]; then
      rm "$image"
    fi
  done
  printf '{"component":"fixkey.release","event":"dmg-create-retry","attempt":%s,"exitCode":%s,"delaySeconds":10}\n' "$attempt" "$status"
  sleep 10
done
