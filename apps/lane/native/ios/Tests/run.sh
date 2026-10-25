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
  -framework Foundation -framework Security -o "${test_output}/credentials-test"
"${test_output}/credentials-test"
