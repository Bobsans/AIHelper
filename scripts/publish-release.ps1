param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $repoRoot

$releaseDir = Join-Path $repoRoot ".release"
$buildDir = Join-Path $releaseDir ".build"
$artifactPath = Join-Path $releaseDir "ah.exe"

if (Test-Path $releaseDir) {
    Remove-Item -LiteralPath $releaseDir -Recurse -Force
}
New-Item -ItemType Directory -Path $buildDir -Force | Out-Null

$previousEnv = @{
    CARGO_TARGET_DIR = $env:CARGO_TARGET_DIR
    CARGO_PROFILE_RELEASE_STRIP = $env:CARGO_PROFILE_RELEASE_STRIP
    CARGO_PROFILE_RELEASE_DEBUG = $env:CARGO_PROFILE_RELEASE_DEBUG
    RUSTFLAGS = $env:RUSTFLAGS
}

try {
    $env:CARGO_TARGET_DIR = $buildDir
    $env:CARGO_PROFILE_RELEASE_STRIP = "symbols"
    $env:CARGO_PROFILE_RELEASE_DEBUG = "0"
    $env:RUSTFLAGS = "-C strip=symbols -C debuginfo=0 -C link-arg=/DEBUG:NONE"

    Write-Host "Building release artifact..."
    cargo build --release --bin ah
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }

    $builtBinary = Join-Path $buildDir "release\ah.exe"
    if (-not (Test-Path $builtBinary)) {
        throw "release binary not found: $builtBinary"
    }

    Copy-Item -LiteralPath $builtBinary -Destination $artifactPath -Force
    Remove-Item -LiteralPath $buildDir -Recurse -Force

    Write-Host "Release artifact ready: $artifactPath"
}
finally {
    $env:CARGO_TARGET_DIR = $previousEnv.CARGO_TARGET_DIR
    $env:CARGO_PROFILE_RELEASE_STRIP = $previousEnv.CARGO_PROFILE_RELEASE_STRIP
    $env:CARGO_PROFILE_RELEASE_DEBUG = $previousEnv.CARGO_PROFILE_RELEASE_DEBUG
    $env:RUSTFLAGS = $previousEnv.RUSTFLAGS
}
