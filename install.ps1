#requires -Version 5.1
# Copyright 2026 Brokk.ai.
# SPDX-License-Identifier: Apache-2.0

[CmdletBinding()]
param(
    [string]$Version,
    [string]$InstallDir
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$repo = 'BrokkAi/muse-acp'
$target = 'x86_64-pc-windows-msvc'

function Say([string]$Message) {
    Write-Output $Message
}

function Fail([string]$Message) {
    throw "muse-acp installer: $Message"
}

function Request-Headers {
    return @{
        Accept = 'application/vnd.github+json'
        'User-Agent' = 'muse-acp-installer'
    }
}

function Download([string]$Uri, [string]$Path) {
    try {
        Invoke-WebRequest -UseBasicParsing -Uri $Uri -Headers (Request-Headers) -OutFile $Path
    }
    catch {
        Fail "could not download $Uri"
    }
}

$tempDir = $null
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

    $tag = $Version
    if ([string]::IsNullOrWhiteSpace($tag)) {
        $tag = $env:MUSE_ACP_VERSION
    }
    if ([string]::IsNullOrWhiteSpace($tag)) {
        try {
            $latest = Invoke-RestMethod -UseBasicParsing -Uri "https://api.github.com/repos/$repo/releases/latest" -Headers (Request-Headers)
            $tag = [string]$latest.tag_name
        }
        catch {
            Fail 'could not determine the latest release'
        }
    }
    if ($tag -notmatch '^v') {
        $tag = "v$tag"
    }
    if ($tag -notmatch '^v[0-9][A-Za-z0-9._+-]*$') {
        Fail "invalid release version: $tag"
    }

    $architecture = $env:PROCESSOR_ARCHITEW6432
    if ([string]::IsNullOrWhiteSpace($architecture)) {
        $architecture = $env:PROCESSOR_ARCHITECTURE
    }
    if ($architecture -notmatch '^(?i:AMD64|X86_64)$') {
        Fail "unsupported architecture: $architecture (the Windows release supports x86_64 only)"
    }

    $configuredDir = $InstallDir
    if ([string]::IsNullOrWhiteSpace($configuredDir)) {
        $configuredDir = $env:MUSE_ACP_INSTALL_DIR
    }
    if ([string]::IsNullOrWhiteSpace($configuredDir)) {
        if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
            Fail 'LOCALAPPDATA is not set; set MUSE_ACP_INSTALL_DIR to an absolute directory'
        }
        $configuredDir = Join-Path $env:LOCALAPPDATA 'Programs\muse-acp'
    }
    if ($configuredDir -notmatch '^(?:[A-Za-z]:[\\/]|\\\\)') {
        Fail "MUSE_ACP_INSTALL_DIR must be an absolute path: $configuredDir"
    }
    $installDir = [IO.Path]::GetFullPath($configuredDir)

    $package = "muse-acp-$tag-$target"
    $archive = "$package.zip"
    $releaseBase = "https://github.com/$repo/releases/download/$tag"

    $tempDir = Join-Path ([IO.Path]::GetTempPath()) ("muse-acp-" + [Guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($tempDir) | Out-Null
    $archivePath = Join-Path $tempDir $archive
    $checksumPath = "$archivePath.sha256"

    Say "Downloading muse-acp $tag for $target..."
    Download "$releaseBase/$archive" $archivePath
    Download "$releaseBase/$archive.sha256" $checksumPath

    $expectedHash = ((Get-Content -LiteralPath $checksumPath -Raw) -split '\s+')[0].Trim().ToLowerInvariant()
    if ($expectedHash -notmatch '^[0-9a-f]{64}$') {
        Fail 'release checksum is malformed'
    }
    $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        Fail "checksum mismatch for $archive; refusing to install"
    }

    Expand-Archive -LiteralPath $archivePath -DestinationPath $tempDir -Force
    $binaryPath = Join-Path $tempDir 'muse-acp.exe'
    if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
        Fail 'release archive does not contain muse-acp.exe'
    }
    $null = & $binaryPath --version 2>$null
    if ($LASTEXITCODE -ne 0) {
        Fail 'the downloaded binary cannot run on this system'
    }

    [IO.Directory]::CreateDirectory($installDir) | Out-Null
    $destination = Join-Path $installDir 'muse-acp.exe'
    $stagedBinary = Join-Path $installDir ('.muse-acp.exe.tmp.' + $PID)
    try {
        Copy-Item -LiteralPath $binaryPath -Destination $stagedBinary -Force
        if (Test-Path -LiteralPath $destination -PathType Leaf) {
            [IO.File]::Replace($stagedBinary, $destination, $null)
        }
        else {
            [IO.File]::Move($stagedBinary, $destination)
        }
    }
    catch {
        if (Test-Path -LiteralPath $stagedBinary) {
            Remove-Item -LiteralPath $stagedBinary -Force -ErrorAction SilentlyContinue
        }
        Fail "cannot write to $installDir; set MUSE_ACP_INSTALL_DIR to a writable directory"
    }

    Say "Installed muse-acp $tag to $destination"
    Say "Add $installDir to your user PATH, then restart PowerShell."
    Say ('For the current PowerShell session: $env:Path += "{0}"' -f ";$installDir")
    Say 'Register it with an editor:'
    Say '  Zed:       muse-acp install'
    Say '  JetBrains: muse-acp install-intellij'
}
catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
finally {
    if ($null -ne $tempDir -and (Test-Path -LiteralPath $tempDir)) {
        Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
