<#
.SYNOPSIS
    Downloads and stages the third-party binaries bundled into the Windows
    installer (T067): adb + DLLs, ffmpeg, and the already-built fork jar.

.DESCRIPTION
    Runs BEFORE `pnpm tauri build` - not part of the app runtime, just a
    build prep step. Idempotent: safe to re-run anytime to refresh the
    vendored binaries.

    Output: installer/windows/vendor/bin/{adb.exe, AdbWinApi.dll,
    AdbWinUsbApi.dll, ffmpeg.exe, scrcpy-server-camlink} - referenced by
    `bundle.resources` in src-tauri/tauri.windows.conf.json.

    ffmpeg comes from BtbN/FFmpeg-Builds (static GPL build, no extra DLLs -
    https://github.com/BtbN/FFmpeg-Builds). Their "latest" tag is rebuilt
    periodically per branch (n8.1, n9.0, master); we pin the n8.1 branch
    (same line used on this machine in dev) instead of "master" for
    stability, but it is not a byte-for-byte immutable artifact - if you
    need full reproducibility, download once and vendor that zip separately.

.NOTES
    Requires the fork jar already built at scrcpy/dist/scrcpy-server-camlink
    (run scrcpy/build-camlink.sh first, via WSL/Git Bash with JDK 17 +
    Android SDK - this script does not build the fork, only vendors
    binaries).

    `-Stub` skips every download and just creates empty placeholder files
    at the same paths, instantly. `tauri-build`'s build.rs checks that
    every `bundle.resources` path in tauri.windows.conf.json EXISTS on
    every `cargo build`/`check`/`clippy`/`test` on Windows (not only when
    actually bundling an installer) - without this, a fresh clone (or CI)
    can't even compile until someone runs the full vendoring. Use `-Stub`
    for plain dev/test work; drop it (or re-run without it) before an
    actual `tauri build` you intend to ship.
#>

[CmdletBinding()]
param(
    [string]$FfmpegAsset = "ffmpeg-n8.1-latest-win64-gpl-8.1.zip",
    [switch]$Stub
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$VendorBin = Join-Path $PSScriptRoot "vendor\bin"
$TmpDir = Join-Path $PSScriptRoot "vendor\.tmp"

New-Item -ItemType Directory -Force -Path $VendorBin | Out-Null

# camlink_lib.dll (the DirectShow filter itself) is also a bundle.resources
# entry, but it's a build OUTPUT (cargo produces it), not something we
# vendor. build.rs checks it exists even before the very first `cargo
# build --release` has produced it (chicken-and-egg on a fresh checkout) -
# a placeholder here is harmless either way: cargo overwrites it with the
# real DLL once the release build actually runs.
$ReleaseDll = Join-Path $RepoRoot "src-tauri\target\release\camlink_lib.dll"
if (-not (Test-Path $ReleaseDll)) {
    New-Item -ItemType Directory -Force -Path (Split-Path $ReleaseDll) | Out-Null
    New-Item -ItemType File -Force -Path $ReleaseDll | Out-Null
}

if ($Stub) {
    foreach ($file in @("adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll", "ffmpeg.exe", "scrcpy-server-camlink")) {
        New-Item -ItemType File -Force -Path (Join-Path $VendorBin $file) | Out-Null
        Write-Host "  -> $file (stub)"
    }
    Write-Host ""
    Write-Host "Stub vendoring done at $VendorBin (empty placeholders, not real binaries)"
    exit 0
}

New-Item -ItemType Directory -Force -Path $TmpDir | Out-Null

function Get-AndExtractZip {
    param(
        [string]$Url,
        [string]$ZipName
    )
    $zipPath = Join-Path $TmpDir $ZipName
    Write-Host "Downloading $Url ..."
    Invoke-WebRequest -Uri $Url -OutFile $zipPath
    $extractDir = Join-Path $TmpDir ([System.IO.Path]::GetFileNameWithoutExtension($ZipName))
    if (Test-Path $extractDir) { Remove-Item -Recurse -Force $extractDir }
    Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force
    return $extractDir
}

# --- adb + DLLs (Google, official) -----------------------------------------
Write-Host "== platform-tools (adb) =="
$ptDir = Get-AndExtractZip `
    -Url "https://dl.google.com/android/repository/platform-tools-latest-windows.zip" `
    -ZipName "platform-tools.zip"
$ptBin = Join-Path $ptDir "platform-tools"
foreach ($file in @("adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll")) {
    $src = Join-Path $ptBin $file
    if (-not (Test-Path $src)) {
        throw "expected $file under $ptBin, not found - did the platform-tools zip layout change?"
    }
    Copy-Item -Force $src (Join-Path $VendorBin $file)
    Write-Host "  -> $file"
}

# --- ffmpeg (BtbN, static GPL build) ---------------------------------------
Write-Host "== ffmpeg =="
$ffDir = Get-AndExtractZip `
    -Url "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/$FfmpegAsset" `
    -ZipName $FfmpegAsset
$ffExe = Get-ChildItem -Path $ffDir -Recurse -Filter "ffmpeg.exe" | Select-Object -First 1
if (-not $ffExe) {
    throw "ffmpeg.exe not found inside $ffDir after extracting $FfmpegAsset"
}
Copy-Item -Force $ffExe.FullName (Join-Path $VendorBin "ffmpeg.exe")
Write-Host "  -> ffmpeg.exe ($($ffExe.FullName))"

# --- fork jar (already built locally, not downloaded) -----------------------
Write-Host "== scrcpy-server-camlink (fork) =="
$jarSrc = Join-Path $RepoRoot "scrcpy\dist\scrcpy-server-camlink"
$jarSha = "$jarSrc.sha256"
if (-not (Test-Path $jarSrc)) {
    throw "scrcpy/dist/scrcpy-server-camlink does not exist. Run scrcpy/build-camlink.sh " +
          "(via WSL/Git Bash, requires JDK 17 + Android SDK) before this script."
}
if (Test-Path $jarSha) {
    $expected = (Get-Content $jarSha -Raw).Trim().Split(" ")[0]
    $actual = (Get-FileHash -Algorithm SHA256 $jarSrc).Hash.ToLower()
    if ($expected -and ($expected.ToLower() -ne $actual)) {
        throw "sha256 of scrcpy-server-camlink does not match $jarSha - rebuild with scrcpy/build-camlink.sh."
    }
}
Copy-Item -Force $jarSrc (Join-Path $VendorBin "scrcpy-server-camlink")
Write-Host "  -> scrcpy-server-camlink"

Remove-Item -Recurse -Force $TmpDir
Write-Host ""
Write-Host "Vendoring done at $VendorBin"
