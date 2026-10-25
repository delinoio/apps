# SPDX-License-Identifier: Apache-2.0
# Include after the shared desktop CMake project creates target Lane on Windows.
if(NOT WIN32 OR NOT TARGET Lane)
  message(FATAL_ERROR "Windows host integration requires the Windows Lane target")
endif()
if(NOT TARGET lane_store)
  add_subdirectory("${CMAKE_CURRENT_LIST_DIR}/../../store" "${CMAKE_CURRENT_BINARY_DIR}/lane-store")
endif()
set(LANE_WINDOWS_APP_SDK_INCLUDE_DIR "" CACHE PATH
  "Official Windows App SDK include directory containing MddBootstrap.h and WindowsAppSDK-VersionInfo.h")
set(LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR "" CACHE PATH
  "Official cppwinrt-generated include directory containing winrt/Microsoft.Windows.AppNotifications.h")
set(LANE_WINDOWS_APP_SDK_BOOTSTRAP_DLL "" CACHE FILEPATH
  "Architecture-matched Microsoft.WindowsAppRuntime.Bootstrap.dll from the same official Windows App SDK")
set(LANE_WINDOWS_APP_SDK_LICENSE "" CACHE FILEPATH "Original license distributed with the official Windows App SDK package")
foreach(header MddBootstrap.h WindowsAppSDK-VersionInfo.h)
  if(NOT EXISTS "${LANE_WINDOWS_APP_SDK_INCLUDE_DIR}/${header}")
    message(FATAL_ERROR "Set LANE_WINDOWS_APP_SDK_INCLUDE_DIR to the official SDK headers (${header} missing)")
  endif()
endforeach()
if(NOT EXISTS "${LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR}/winrt/Microsoft.Windows.AppNotifications.h")
  message(FATAL_ERROR "Generate Windows App SDK C++/WinRT projections and set LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR")
endif()
if(NOT EXISTS "${LANE_WINDOWS_APP_SDK_BOOTSTRAP_DLL}")
  message(FATAL_ERROR "Set LANE_WINDOWS_APP_SDK_BOOTSTRAP_DLL to the matching official SDK bootstrap DLL")
endif()
if(NOT EXISTS "${LANE_WINDOWS_APP_SDK_LICENSE}")
  message(FATAL_ERROR "Set LANE_WINDOWS_APP_SDK_LICENSE to the original SDK package license")
endif()
target_include_directories(Lane PRIVATE "${LANE_WINDOWS_APP_SDK_INCLUDE_DIR}" "${LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR}")
target_compile_definitions(Lane PRIVATE USE_WEAK_SUFFIX_NAPI "$<$<CONFIG:Debug>:LANE_DEVELOPMENT>")
target_link_libraries(Lane PRIVATE lane_store windowsapp runtimeobject ole32 shell32 advapi32 bcrypt)
if(MSVC)
  target_compile_options(Lane PRIVATE /utf-8 /EHsc /permissive-)
endif()
add_custom_target(LaneWindowsRuntime ALL
  COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${LANE_WINDOWS_APP_SDK_BOOTSTRAP_DLL}"
    "$<TARGET_FILE_DIR:Lane>/Microsoft.WindowsAppRuntime.Bootstrap.dll"
  DEPENDS Lane VERBATIM)
add_custom_command(TARGET LaneWindowsRuntime POST_BUILD
  COMMAND "${CMAKE_COMMAND}" -E make_directory "$<TARGET_FILE_DIR:Lane>/licenses/windows-app-sdk"
  COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${LANE_WINDOWS_APP_SDK_LICENSE}"
    "$<TARGET_FILE_DIR:Lane>/licenses/windows-app-sdk/LICENSE"
  VERBATIM)
foreach(notice IN ITEMS sqlite/LICENSE ada/LICENSE-APACHE ada/LICENSE-MIT
    json/LICENSE.MIT picosha2/LICENSE utf8proc/LICENSE.md)
  get_filename_component(owner "${notice}" DIRECTORY)
  add_custom_command(TARGET LaneWindowsRuntime POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E make_directory "$<TARGET_FILE_DIR:Lane>/licenses/lane-store/${owner}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${CMAKE_CURRENT_LIST_DIR}/../../store/vendor/${notice}"
      "$<TARGET_FILE_DIR:Lane>/licenses/lane-store/${notice}"
    VERBATIM)
endforeach()
# The shared coordinator embeds the matching packages/lane artifact at
# $<TARGET_FILE_DIR:Lane>/helpers/lane.exe. Lane.exe and lane.exe cannot coexist
# in one Windows directory. Do not search PATH or invoke a global npm package.
