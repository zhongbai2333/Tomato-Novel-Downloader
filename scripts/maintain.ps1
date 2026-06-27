<#
.SYNOPSIS
Runs the local maintenance checks for Tomato Novel Downloader.

.DESCRIPTION
This script intentionally avoids `cargo --all-features` because the project has
mutually exclusive features: `official-api` and `no-official-api`.

Run from the repository root:
  pwsh ./scripts/maintain.ps1
  powershell -ExecutionPolicy Bypass -File ./scripts/maintain.ps1
#>

param(
    [switch]$SkipFmt,
    [switch]$SkipNoOfficial,
    [switch]$SkipTree
)

$ErrorActionPreference = "Stop"

function Invoke-Step {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [scriptblock]$Script
    )

    Write-Host "`n==> $Name" -ForegroundColor Cyan
    & $Script
}

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

Invoke-Step "Rust toolchain" {
    rustc --version
    cargo --version
}

if (-not $SkipFmt) {
    Invoke-Step "Format check" {
        cargo fmt --all -- --check
    }
}

Invoke-Step "Default feature tests" {
    cargo test
}

Invoke-Step "Default feature clippy" {
    cargo clippy --all-targets -- -D warnings
}

if (-not $SkipNoOfficial) {
    Invoke-Step "no-official-api tests" {
        cargo test --no-default-features --features no-official-api
    }

    Invoke-Step "no-official-api clippy" {
        cargo clippy --no-default-features --features no-official-api --all-targets -- -D warnings
    }
}

if (-not $SkipTree) {
    Invoke-Step "Duplicate dependency overview" {
        cargo tree -d
    }
}

Write-Host "`nAll requested maintenance checks completed." -ForegroundColor Green
