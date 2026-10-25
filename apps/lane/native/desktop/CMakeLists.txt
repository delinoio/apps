# SPDX-License-Identifier: Apache-2.0
cmake_minimum_required(VERSION 3.24)
project(Lane VERSION 0.1.0 LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(LYNX_SDK "$ENV{LANE_LYNX_SDK}" CACHE PATH "Extracted official Lynx 4.1.0 SDK")
if(NOT EXISTS "${LYNX_SDK}/include/lynx_view.h")
  message(FATAL_ERROR "Set LANE_LYNX_SDK to an extracted Lynx 4.1.0 desktop SDK")
endif()
set(LANE_URL_SCHEME "lane-dev" CACHE STRING "Registered URL scheme (lane-dev or lane)")
if(NOT LANE_URL_SCHEME MATCHES "^(lane-dev|lane)$")
  message(FATAL_ERROR "LANE_URL_SCHEME must be lane-dev or lane")
endif()
add_subdirectory(../store "${CMAKE_CURRENT_BINARY_DIR}/store")
option(LANE_NATIVE_TESTING "Allow explicit synthetic native test roots" OFF)
set(LANE_SYNC_HELPER "" CACHE FILEPATH "Matching packaged Lane CLI executable")
set(LANE_DIST "${CMAKE_CURRENT_SOURCE_DIR}/../../dist")
if(NOT EXISTS "${LANE_DIST}/main.lynx.bundle")
  message(FATAL_ERROR "Run pnpm build from apps/lane first")
endif()
if(APPLE)
  enable_language(OBJCXX)
  set(CMAKE_OBJCXX_STANDARD 17)
  set(CMAKE_OBJCXX_STANDARD_REQUIRED ON)
  configure_file(macos/Info.plist.in "${CMAKE_CURRENT_BINARY_DIR}/Lane-Info.plist" @ONLY)
  add_executable(Lane MACOSX_BUNDLE macos.mm macos/runtime.mm)
  target_compile_options(Lane PRIVATE -fobjc-arc)
  target_link_libraries(Lane PRIVATE "${LYNX_SDK}/lib/libLynx.dylib" "-framework Cocoa" "-framework UserNotifications")
  set_target_properties(Lane PROPERTIES
    MACOSX_BUNDLE_INFO_PLIST "${CMAKE_CURRENT_BINARY_DIR}/Lane-Info.plist"
    MACOSX_BUNDLE_GUI_IDENTIFIER "io.delino.lane"
    MACOSX_BUNDLE_BUNDLE_NAME "Lane"
    MACOSX_BUNDLE_SHORT_VERSION_STRING "0.1.0"
    MACOSX_BUNDLE_BUNDLE_VERSION "1"
    BUILD_WITH_INSTALL_RPATH TRUE
    INSTALL_RPATH "@executable_path/../Frameworks")
  set(LANE_RESOURCES "$<TARGET_BUNDLE_CONTENT_DIR:Lane>/Resources")
  add_custom_command(TARGET Lane POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E make_directory "$<TARGET_BUNDLE_CONTENT_DIR:Lane>/Frameworks"
    COMMAND "${CMAKE_COMMAND}" -E copy_directory "${LYNX_SDK}/lib" "$<TARGET_BUNDLE_CONTENT_DIR:Lane>/Frameworks"
    COMMAND "${CMAKE_COMMAND}" -E copy_directory "${LYNX_SDK}/bundles" "${LANE_RESOURCES}"
    COMMAND "${CMAKE_COMMAND}" -E copy_directory "${LYNX_SDK}/data" "${LANE_RESOURCES}"
    VERBATIM)
elseif(WIN32)
  add_executable(Lane WIN32 windows.cc)
  include("${CMAKE_CURRENT_SOURCE_DIR}/windows/integration.cmake")
  target_compile_definitions(Lane PRIVATE UNICODE _UNICODE _WIN32_WINNT=0x0A00)
  target_link_libraries(Lane PRIVATE "${LYNX_SDK}/lib/lynx.dll.lib")
  set(LANE_RESOURCES "$<TARGET_FILE_DIR:Lane>")
  add_custom_command(TARGET Lane POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E copy_directory "${LYNX_SDK}/lib" "${LANE_RESOURCES}"
    COMMAND "${CMAKE_COMMAND}" -E copy_directory "${LYNX_SDK}/data" "${LANE_RESOURCES}/data"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${LYNX_SDK}/lynx_core.js" "${LANE_RESOURCES}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${LYNX_SDK}/lynx_core_dev.js" "${LANE_RESOURCES}"
    VERBATIM)
else()
  message(FATAL_ERROR "Native Lane desktop supports macOS and Windows; use web on Linux")
endif()
target_link_libraries(Lane PRIVATE lane_store)
target_compile_definitions(Lane PRIVATE USE_WEAK_SUFFIX_NAPI)
if(LANE_NATIVE_TESTING)
  target_compile_definitions(Lane PRIVATE LANE_NATIVE_TESTING)
  if(APPLE)
    enable_testing()
    find_package(Python3 REQUIRED COMPONENTS Interpreter)
    add_executable(LaneReauthTest macos/reauth_test.mm macos/runtime.mm)
    target_compile_options(LaneReauthTest PRIVATE -fobjc-arc)
    target_compile_definitions(LaneReauthTest PRIVATE LANE_NATIVE_TESTING USE_WEAK_SUFFIX_NAPI)
    target_include_directories(LaneReauthTest PRIVATE "${LYNX_SDK}/include")
    target_link_libraries(LaneReauthTest PRIVATE lane_store "-framework Cocoa" "-framework UserNotifications")
    add_test(NAME lane_macos_reauthentication COMMAND "${Python3_EXECUTABLE}"
      "${CMAKE_CURRENT_SOURCE_DIR}/macos/run_reauth_test.py" "$<TARGET_FILE:LaneReauthTest>")
  endif()
endif()
if(LANE_SYNC_HELPER)
  if(NOT EXISTS "${LANE_SYNC_HELPER}")
    message(FATAL_ERROR "LANE_SYNC_HELPER does not exist")
  endif()
  get_filename_component(LANE_HELPER_SOURCE_DIR "${LANE_SYNC_HELPER}" DIRECTORY)
  foreach(notice IN ITEMS LICENSE NOTICE THIRD_PARTY_NOTICES)
    if(NOT EXISTS "${LANE_HELPER_SOURCE_DIR}/${notice}")
      message(FATAL_ERROR "The packaged Lane helper requires adjacent ${notice}")
    endif()
  endforeach()
  if(APPLE)
    set(LANE_HELPER_DEST "$<TARGET_BUNDLE_CONTENT_DIR:Lane>/MacOS/helpers/lane")
  else()
    set(LANE_HELPER_DEST "$<TARGET_FILE_DIR:Lane>/helpers/lane.exe")
  endif()
  add_custom_target(LaneSyncHelper ALL
    COMMAND "${CMAKE_COMMAND}" -E make_directory "$<TARGET_FILE_DIR:Lane>/helpers"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${LANE_SYNC_HELPER}" "${LANE_HELPER_DEST}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${LANE_HELPER_SOURCE_DIR}/LICENSE"
      "${LANE_HELPER_SOURCE_DIR}/NOTICE" "$<TARGET_FILE_DIR:Lane>/helpers"
    COMMAND "${CMAKE_COMMAND}" -E copy_directory "${LANE_HELPER_SOURCE_DIR}/THIRD_PARTY_NOTICES"
      "$<TARGET_FILE_DIR:Lane>/helpers/THIRD_PARTY_NOTICES"
    VERBATIM)
  add_dependencies(LaneSyncHelper Lane)
else()
  message(STATUS "Lane sync helper unavailable: standalone local mode remains supported")
endif()
target_include_directories(Lane PRIVATE "${LYNX_SDK}/include")
# The template changes independently of the native executable. Run resource
# synchronization in every default build, including when resources are missing.
add_custom_target(LaneResources ALL
  COMMAND "${CMAKE_COMMAND}" -E copy_if_different
    "${LANE_DIST}/main.lynx.bundle" "${LANE_DIST}/LICENSE" "${LANE_DIST}/NOTICE" "${LANE_RESOURCES}"
  COMMAND "${CMAKE_COMMAND}" -E copy_directory "${CMAKE_CURRENT_SOURCE_DIR}/../licenses" "${LANE_RESOURCES}/licenses"
  VERBATIM)
add_dependencies(LaneResources Lane)

if(APPLE)
  foreach(notice IN ITEMS sqlite/LICENSE ada/LICENSE-APACHE ada/LICENSE-MIT
      json/LICENSE.MIT picosha2/LICENSE utf8proc/LICENSE.md)
    get_filename_component(owner "${notice}" DIRECTORY)
    add_custom_command(TARGET LaneResources POST_BUILD
      COMMAND "${CMAKE_COMMAND}" -E make_directory "${LANE_RESOURCES}/licenses/lane-store/${owner}"
      COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${CMAKE_CURRENT_SOURCE_DIR}/../store/vendor/${notice}"
        "${LANE_RESOURCES}/licenses/lane-store/${notice}"
      VERBATIM)
  endforeach()
endif()
