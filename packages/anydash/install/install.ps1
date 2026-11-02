# SPDX-License-Identifier: Apache-2.0
#
# Installs the AnyDash CLI on Windows 11 (x64 and arm64):
#
#   irm https://anydash.delino.io/install.ps1 | iex
#
# The script downloads the archive for this CPU and SHA256SUMS from the AnyDash
# GitHub Release, verifies the checksum before extracting, and installs
# anydash.exe into %LOCALAPPDATA%\Programs\AnyDash without elevated privileges.
# It adds that directory to the user PATH. Run it again to update.
#
# Environment:
#   ANYDASH_VERSION       install this release (MAJOR.MINOR.PATCH) instead of the latest
#   ANYDASH_INSTALL_DIR   install into this directory
#   ANYDASH_NO_MODIFY_PATH=1
#                         leave the user PATH unchanged
#   ANYDASH_RELEASES_URL, ANYDASH_DOWNLOAD_URL
#                         release list and download base, for mirrors and tests
#                         (default: GitHub Releases); the script appends per_page
#                         and page parameters

function Get-AnyDashTarget {
  param([Parameter(Mandatory)][string]$Architecture)
  switch ($Architecture) {
    'X64' { return 'windows-x64' }
    'Arm64' { return 'windows-arm64' }
    default { throw "unsupported platform windows/$Architecture; AnyDash supports x64 and arm64" }
  }
}

function Get-AnyDashListedChecksum {
  param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Name)
  $found = @()
  foreach ($line in Get-Content -LiteralPath $Path) {
    if ($line -match '^([a-f0-9]{64}) [ *](.+)$' -and $Matches[2] -eq $Name) { $found += $Matches[1] }
  }
  if ($found.Count -ne 1) { throw "SHA256SUMS does not list $Name exactly once" }
  return $found[0]
}

function Get-AnyDashDigest {
  param([Parameter(Mandatory)][string]$Path)
  return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# The repository's other products publish releases too, and GitHub does not
# document the order of the release list, so this reads every page up to
# MaxPages (1000 releases by default) before choosing the highest version.
function Get-AnyDashLatestVersion {
  param([Parameter(Mandatory)][string]$ReleasesUrl, [int]$PageSize = 100, [int]$MaxPages = 10)
  $ProgressPreference = 'SilentlyContinue'
  $separator = if ($ReleasesUrl.Contains('?')) { '&' } else { '?' }
  $latest = $null
  for ($page = 1; $page -le $MaxPages; $page++) {
    $response = Invoke-RestMethod -Uri "$ReleasesUrl${separator}per_page=$PageSize&page=$page" -UseBasicParsing `
      -Headers @{ Accept = 'application/vnd.github+json' }
    # Assign before enumerating: Windows PowerShell 5.1 emits a JSON array as one object.
    $releases = @($response | Where-Object { $null -ne $_ })
    foreach ($release in $releases) {
      if ($release.draft -or $release.prerelease) { continue }
      if ($release.tag_name -notmatch '^anydash@v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$') { continue }
      $candidate = [version]::new([int]$Matches[1], [int]$Matches[2], [int]$Matches[3])
      if ($null -eq $latest -or $candidate -gt $latest) { $latest = $candidate }
    }
    # A short page is the last one.
    if ($releases.Count -lt $PageSize) { break }
    if ($page -eq $MaxPages) {
      Write-Host "anydash install: the release list has more than $MaxPages pages; only those were searched"
    }
  }
  if ($null -eq $latest) { throw 'no published AnyDash release was found' }
  return $latest.ToString()
}

function Install-AnyDash {
  param(
    [Parameter(Mandatory)][string]$Target,
    [string]$Version,
    [Parameter(Mandatory)][string]$InstallDir,
    [Parameter(Mandatory)][string]$ReleasesUrl,
    [Parameter(Mandatory)][string]$DownloadUrl,
    [bool]$UpdatePath = $true
  )
  $ErrorActionPreference = 'Stop'
  $ProgressPreference = 'SilentlyContinue'
  if (-not $Version) { $Version = Get-AnyDashLatestVersion -ReleasesUrl $ReleasesUrl }
  $Version = $Version -replace '^v', ''
  if ($Version -notmatch '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$') {
    throw "invalid version $Version; expected MAJOR.MINOR.PATCH"
  }
  $archive = "anydash-$Target-$Version.tar.gz"
  $base = "$DownloadUrl/anydash@v$Version"
  Write-Host "anydash install: installing AnyDash $Version for $Target"
  $work = Join-Path ([System.IO.Path]::GetTempPath()) ("anydash-install-" + [guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $work | Out-Null
  try {
    try { Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile (Join-Path $work 'SHA256SUMS') -UseBasicParsing }
    catch { throw "cannot download SHA256SUMS for AnyDash $Version" }
    try { Invoke-WebRequest -Uri "$base/$archive" -OutFile (Join-Path $work $archive) -UseBasicParsing }
    catch { throw "cannot download $archive" }
    $expected = Get-AnyDashListedChecksum -Path (Join-Path $work 'SHA256SUMS') -Name $archive
    if ((Get-AnyDashDigest -Path (Join-Path $work $archive)) -ne $expected) { throw "checksum mismatch for $archive" }
    $extract = Join-Path $work 'extract'
    New-Item -ItemType Directory -Path $extract | Out-Null
    # Windows 10 and later include tar.exe. Prefer it over other tar commands on
    # PATH: GNU tar reads drive-qualified archive paths as remote hosts.
    $tar = 'tar'
    if ($env:SystemRoot -and (Test-Path -LiteralPath (Join-Path $env:SystemRoot 'System32\tar.exe'))) {
      $tar = Join-Path $env:SystemRoot 'System32\tar.exe'
    }
    & $tar -xzf (Join-Path $work $archive) -C $extract
    if ($LASTEXITCODE -ne 0) { throw "cannot extract $archive" }
    $package = Join-Path $extract "anydash-$Target"
    $executable = Join-Path $package 'anydash.exe'
    $item = Get-Item -LiteralPath $executable -ErrorAction SilentlyContinue
    if (-not $item -or $item.PSIsContainer -or $item.LinkType) { throw "$archive does not contain anydash.exe" }
    $inner = Get-AnyDashListedChecksum -Path (Join-Path $package 'SHA256SUMS') -Name 'anydash.exe'
    if ((Get-AnyDashDigest -Path $executable) -ne $inner) { throw 'checksum mismatch for the extracted executable' }
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $staged = Join-Path $InstallDir ".anydash.install.$PID.exe"
    Copy-Item -LiteralPath $executable -Destination $staged -Force
    try { Move-Item -LiteralPath $staged -Destination (Join-Path $InstallDir 'anydash.exe') -Force }
    catch {
      Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
      throw "cannot replace $(Join-Path $InstallDir 'anydash.exe'); stop running anydash processes and retry"
    }
  }
  finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
  }
  Write-Host "anydash install: installed AnyDash $Version to $(Join-Path $InstallDir 'anydash.exe')"
  if ($UpdatePath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($userPath -split ';' | Where-Object { $_ })
    if ($entries -notcontains $InstallDir) {
      [Environment]::SetEnvironmentVariable('Path', (($entries + $InstallDir) -join ';'), 'User')
      $env:Path = "$env:Path;$InstallDir"
      Write-Host "anydash install: added $InstallDir to the user PATH; open a new terminal to use it"
    }
  }
  Write-Host "anydash install: next, run 'anydash login' to connect this device"
}

function Invoke-AnyDashInstall {
  $ErrorActionPreference = 'Stop'
  try {
    if ($env:OS -ne 'Windows_NT') {
      throw 'unsupported platform; use https://anydash.delino.io/install.sh on macOS and Linux'
    }
    $architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    $installDir = if ($env:ANYDASH_INSTALL_DIR) { $env:ANYDASH_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\AnyDash' }
    $releasesUrl = if ($env:ANYDASH_RELEASES_URL) { $env:ANYDASH_RELEASES_URL } else { 'https://api.github.com/repos/delinoio/apps/releases' }
    $downloadUrl = if ($env:ANYDASH_DOWNLOAD_URL) { $env:ANYDASH_DOWNLOAD_URL } else { 'https://github.com/delinoio/apps/releases/download' }
    # Windows PowerShell 5.1 may default to older TLS versions.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    Install-AnyDash -Target (Get-AnyDashTarget -Architecture $architecture) -Version $env:ANYDASH_VERSION `
      -InstallDir $installDir -ReleasesUrl $releasesUrl -DownloadUrl $downloadUrl `
      -UpdatePath ($env:ANYDASH_NO_MODIFY_PATH -ne '1')
  }
  catch {
    Write-Host "anydash install: error: $($_.Exception.Message)" -ForegroundColor Red
    Write-Host 'anydash install: nothing was installed' -ForegroundColor Red
    throw
  }
}

# Tests dot-source this file with $AnyDashInstallLibrary set to load the functions only.
if (-not (Get-Variable -Name AnyDashInstallLibrary -ValueOnly -ErrorAction SilentlyContinue)) {
  Invoke-AnyDashInstall
}
