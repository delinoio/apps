# SPDX-License-Identifier: Apache-2.0
cmake_minimum_required(VERSION 3.22.1)
project(lane_android LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
# The shared target owns the patched SQLite dependency and its notices.
set(LANE_STORE_ROOT "${CMAKE_CURRENT_LIST_DIR}/../../../../../store" CACHE PATH "Lane common engine source directory")
if(NOT EXISTS "${LANE_STORE_ROOT}/CMakeLists.txt")
  message(FATAL_ERROR "Integrate apps/lane/native/store (lane_store target) before building Android")
endif()
add_subdirectory("${LANE_STORE_ROOT}" "${CMAKE_CURRENT_BINARY_DIR}/store")
add_library(lane_android SHARED lane_jni.cc lane_log.cc)
target_include_directories(lane_android PRIVATE "${LANE_STORE_ROOT}")
target_link_libraries(lane_android PRIVATE lane_store)
