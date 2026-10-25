#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu
store_source="${SRCROOT}/../store"
store_build="${DERIVED_FILE_DIR}/lane-store-${PLATFORM_NAME}"
store_archs=$(printf '%s' "${ARCHS}" | tr ' ' ';')
cmake -S "${store_source}" -B "${store_build}" \
  -DCMAKE_SYSTEM_NAME=iOS \
  -DCMAKE_OSX_SYSROOT="${SDKROOT}" \
  -DCMAKE_OSX_ARCHITECTURES="${store_archs}" \
  -DCMAKE_OSX_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET}" \
  -DCMAKE_BUILD_TYPE="${CONFIGURATION}" \
  -DCMAKE_ARCHIVE_OUTPUT_DIRECTORY="${LANE_STORE_OUTPUT}" \
  -DLANE_STORE_TESTS=OFF
cmake --build "${store_build}" --target lane_store --parallel
