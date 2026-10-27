#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu
test_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
test_output=$(mktemp -d "${TMPDIR:-/tmp}/lane-ios-tests.XXXXXX")
trap 'rm -rf "${test_output}"' EXIT HUP INT TERM
xcrun --sdk macosx clang++ -std=c++17 -fobjc-arc -Wall -Wextra -Wno-unused-parameter \
  -I "${test_root}/../Sources" \
  "${test_root}/credentials-test.mm" \
  "${test_root}/../Sources/LaneCredentials.mm" \
  "${test_root}/../Sources/LaneTransport.mm" \
  "${test_root}/../Sources/LaneReauthentication.mm" \
  "${test_root}/../Sources/LaneDeletion.mm" \
  -framework Foundation -framework Security -o "${test_output}/credentials-test"
"${test_output}/credentials-test"
cmake -S "${test_root}/../../store" -B "${test_output}/store" -DLANE_STORE_TESTS=ON
cmake --build "${test_output}/store" --target lane_store -j 4
xcrun --sdk macosx clang++ -std=c++17 -fobjc-arc -Wall -Wextra -Wno-unused-parameter \
  -I "${test_root}/../Sources" -I "${test_root}/../../store" \
  "${test_root}/sync-upload-test.mm" "${test_root}/../Sources/LaneSyncUpload.mm" \
  "${test_output}/store/liblane_store.a" "${test_output}/store/liblane_sqlite.a" \
  "${test_output}/store/liblane_unicode.a" "${test_output}/store/liblane_url.a" \
  -framework Foundation -o "${test_output}/sync-upload-test"
"${test_output}/sync-upload-test"
xcrun --sdk macosx clang++ -std=c++17 -fobjc-arc -Wall -Wextra -Wno-unused-parameter \
  -I "${test_root}/../Sources" \
  "${test_root}/image-data-test.mm" \
  "${test_root}/../Sources/LaneHTTPSImageData.mm" \
  -framework Foundation -o "${test_output}/image-data-test"
"${test_output}/image-data-test"
