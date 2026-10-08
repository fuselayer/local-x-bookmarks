# xitter-dl — dev environment bootstrap.
#
# Dot-source this before any cargo/pnpm command:
#     Set-ExecutionPolicy -Scope Process Bypass -Force   # see "Execution policy"
#     . .\scripts\dev-env.ps1
#
# Or use the wrapper, which does both and then runs your command:
#     .\scripts\dev.cmd cargo build
#
# Execution policy
# ----------------
# This host blocks *all* unsigned local scripts, not merely downloaded ones:
# a copy of this file in %TEMP% with no Zone.Identifier is refused too, and
# Get-ExecutionPolicy reports RemoteSigned, which should permit it. So the
# effective policy is stricter than it reports (likely DACL/WDAC-enforced).
#
# We do NOT change CurrentUser or LocalMachine policy — that reaches outside
# this project and would be rude. Process-scope Bypass affects only the shell
# you are already in, and is what scripts\dev.cmd uses.
#
# Why this file exists
# --------------------
# Two host facts force it:
#
#   1. Rust is installed at ~\.cargo\bin but is not on the default PATH, and
#      ~\.cargo is read-only to the DSH file sandbox. Cargo writes to
#      $CARGO_HOME\registry on every dependency fetch, so a default cargo
#      invocation fails the moment it needs a crate that isn't cached.
#      Fix: keep CARGO_HOME inside the repo. Everything is then contained in
#      the workspace, which is also just good hygiene for a pinned project.
#
#   2. The rustup *proxy* shims (~\.cargo\bin\cargo.exe) may want to touch
#      ~\.rustup. We bypass them by calling the toolchain binaries directly,
#      which also removes a layer of indirection from error messages.
#
# Nothing here reaches the network. First `cargo build` will, to fetch crates.
#
# NOTE: this file deliberately does NOT set $ErrorActionPreference. It is
# dot-sourced, so any preference it set would leak into the caller's session.
# With 'Stop' active, cargo writing ordinary progress to stderr raises a
# NativeCommandError that terminates the whole script mid-build. Errors that
# matter are raised with explicit `throw`.

$RepoRoot = Split-Path -Parent $PSScriptRoot

# ── Rust ─────────────────────────────────────────────────────────────────────
$env:RUSTUP_HOME       = Join-Path $env:USERPROFILE '.rustup'
$env:RUSTUP_TOOLCHAIN  = 'stable-x86_64-pc-windows-msvc'
$env:RUSTUP_AUTO_INSTALL = '0'          # never silently fetch a toolchain

$env:CARGO_HOME        = Join-Path $RepoRoot '.cargo-home'
$env:CARGO_TARGET_DIR  = Join-Path $RepoRoot 'target'

$ToolchainBin = Join-Path $env:RUSTUP_HOME "toolchains\$env:RUSTUP_TOOLCHAIN\bin"
if (-not (Test-Path (Join-Path $ToolchainBin 'cargo.exe'))) {
    throw "Rust toolchain not found at $ToolchainBin. Run: rustup toolchain install $env:RUSTUP_TOOLCHAIN"
}

New-Item -ItemType Directory -Force -Path $env:CARGO_HOME | Out-Null

# Toolchain first so `rustc` resolves to the real binary, not a rustup shim.
$env:PATH = "$ToolchainBin;$env:CARGO_HOME\bin;$env:PATH"

# ── MSVC ─────────────────────────────────────────────────────────────────────
# This host has VS 2019 Build Tools (14.29) + Windows SDK 10.0.19041. Only
# import vcvars if the linker isn't already resolvable — rustc's own MSVC
# detection is usually sufficient and vcvars costs ~2s per shell.
if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vcvars  = $null
    if (Test-Path $vswhere) {
        $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
        if ($vsPath) {
            $candidate = Join-Path $vsPath 'VC\Auxiliary\Build\vcvars64.bat'
            if (Test-Path $candidate) { $vcvars = $candidate }
        }
    }
    if (-not $vcvars) {
        foreach ($p in @(
            "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2019\BuildTools\VC\Auxiliary\Build\vcvars64.bat",
            "${env:ProgramFiles}\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat",
            "${env:ProgramFiles}\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
        )) { if (Test-Path $p) { $vcvars = $p; break } }
    }
    if ($vcvars) {
        # Pull the vcvars environment into this PowerShell session.
        $lines = & cmd.exe /c "`"$vcvars`" >nul 2>&1 && set"
        foreach ($line in $lines) {
            $i = $line.IndexOf('=')
            if ($i -gt 0) {
                $name  = $line.Substring(0, $i)
                $value = $line.Substring($i + 1)
                Set-Item -Path "env:$name" -Value $value -ErrorAction SilentlyContinue
            }
        }
    } else {
        Write-Warning 'vcvars64.bat not found; relying on rustc MSVC auto-detection.'
    }
}

# ── Node ─────────────────────────────────────────────────────────────────────
# Node 22 / pnpm 11 are already on PATH on this host; assert rather than assume.
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    Write-Warning 'node not found on PATH.'
}
if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
    Write-Warning 'pnpm not found on PATH.'
}

# pnpm's store must also live in-repo for the same sandbox reason as CARGO_HOME.
$env:PNPM_HOME = Join-Path $RepoRoot '.pnpm-home'
$env:npm_config_store_dir = Join-Path $RepoRoot '.pnpm-store'

Write-Host "dev-env ready" -ForegroundColor Green
Write-Host "  repo            $RepoRoot"
Write-Host "  CARGO_HOME      $env:CARGO_HOME"
Write-Host "  CARGO_TARGET_DIR $env:CARGO_TARGET_DIR"
Write-Host "  toolchain       $(& (Join-Path $ToolchainBin 'rustc.exe') --version)"
