# SPDX-License-Identifier: Apache-2.0
$ErrorActionPreference = "Stop"
$destination = Join-Path $env:RUNNER_TEMP "lane-windows-sdk"
New-Item -ItemType Directory -Force $destination | Out-Null

function Get-VerifiedPackage($name, $version, $sha256) {
    $archive = Join-Path $destination "$name.zip"
    $directory = Join-Path $destination $name
    Invoke-WebRequest "https://api.nuget.org/v3-flatcontainer/$name/$version/$name.$version.nupkg" -OutFile $archive
    if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sha256) {
        throw "Package checksum mismatch: $name $version"
    }
    Expand-Archive $archive -DestinationPath $directory -Force
    return $directory
}

$sdk = Get-VerifiedPackage "microsoft.windowsappsdk" "1.7.260224002" "1f28e5cac92c59bbfcf2767240bcfa302cc38e069f6ea9cd579cd8c1f0ed6380"
$cppwinrt = Get-VerifiedPackage "microsoft.windows.cppwinrt" "3.0.260818.1" "a8993608ae9263a8288e1a1d9ee2684a5632f4acc59e1e89ddd86b3b889a7367"
$projections = Join-Path $destination "projections"
# Use the hosted Windows SDK for OS metadata and the pinned App SDK for its
# projections. Never substitute fabricated headers for notification APIs.
& "$cppwinrt/bin/cppwinrt.exe" -input "$sdk/lib/uap10.0" "$sdk/lib/uap10.0.18362" -reference sdk -output $projections
if ($LASTEXITCODE -ne 0) { throw "Cannot generate Windows App SDK projections" }
foreach ($name in @("AppNotifications", "AppLifecycle")) {
    if (!(Test-Path "$projections/winrt/Microsoft.Windows.$name.h")) { throw "Missing $name projection" }
}
# Retain the original license for generated C++/WinRT headers with the SDK
# license that the desktop packaging helper already copies into the archive.
$licenses = Join-Path $destination "LICENSES.txt"
Get-Content "$sdk/license.txt", "$cppwinrt/LICENSE" | Set-Content $licenses
"LANE_WINDOWS_APP_SDK_INCLUDE_DIR=$sdk/include" >> $env:GITHUB_ENV
"LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR=$projections" >> $env:GITHUB_ENV
"LANE_WINDOWS_APP_SDK_BOOTSTRAP_DLL=$sdk/runtimes/win-x64/native/Microsoft.WindowsAppRuntime.Bootstrap.dll" >> $env:GITHUB_ENV
"LANE_WINDOWS_APP_SDK_LICENSE=$licenses" >> $env:GITHUB_ENV
