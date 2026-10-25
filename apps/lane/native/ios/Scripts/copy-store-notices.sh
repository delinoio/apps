#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu
store_vendor="${SRCROOT}/../store/vendor"
notice_root="${TARGET_BUILD_DIR}/${UNLOCALIZED_RESOURCES_FOLDER_PATH}/StoreLicenses"
for notice in sqlite/LICENSE ada/LICENSE-APACHE ada/LICENSE-MIT json/LICENSE.MIT picosha2/LICENSE utf8proc/LICENSE.md; do
  notice_folder=$(dirname -- "${notice}")
  mkdir -p "${notice_root}/${notice_folder}"
  cp "${store_vendor}/${notice}" "${notice_root}/${notice}"
done
