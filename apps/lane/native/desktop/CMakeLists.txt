# SPDX-License-Identifier: Apache-2.0
cmake_minimum_required(VERSION 3.24)
project(Lane VERSION 0.1.0 LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(LYNX_SDK "$ENV{LANE_LYNX_SDK}" CACHE PATH "Extracted official Lynx 4.1.0 SDK")
if(NOT EXISTS "${LYNX_SDK}/include/lynx_view.h")
  message(FATAL_ERROR "Set LANE_LYNX_SDK to an extracted Lynx 4.1.0 desktop SDK")
endif()
set(LANE_DIST "${CMAKE_CURRENT_SOURCE_DIR}/../../dist")
if(NOT EXISTS "${LANE_DIST}/main.lynx.bundle")
  message(FATAL_ERROR "Run pnpm build from apps/lane first")
endif()
if(APPLE)
  enable_language(OBJCXX)
  set(CMAKE_OBJCXX_STANDARD 17)
  set(CMAKE_OBJCXX_STANDARD_REQUIRED ON)
  add_executable(Lane MACOSX_BUNDLE macos.mm)
  target_compile_options(Lane PRIVATE -fobjc-arc)
  target_link_libraries(Lane PRIVATE "${LYNX_SDK}/lib/libLynx.dylib" "-framework Cocoa")
  set_target_properties(Lane PROPERTIES
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
target_include_directories(Lane PRIVATE "${LYNX_SDK}/include")
add_custom_command(TARGET Lane POST_BUILD
  COMMAND "${CMAKE_COMMAND}" -E copy_if_different
    "${LANE_DIST}/main.lynx.bundle" "${LANE_DIST}/LICENSE" "${LANE_DIST}/NOTICE" "${LANE_RESOURCES}"
  COMMAND "${CMAKE_COMMAND}" -E copy_directory "${CMAKE_CURRENT_SOURCE_DIR}/../licenses" "${LANE_RESOURCES}/licenses"
  VERBATIM)
