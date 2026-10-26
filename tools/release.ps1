# Local Rustcraft release, without GitHub Actions (no minutes used): checks, builds, packages,
# computes the fingerprint, then publishes the release with `gh` (GitHub API).
#
# Usage (from the repository root):
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\release.ps1 -Version 0.1.0          # everything, then publish
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\release.ps1 -Version 0.1.0 -DryRun  # without publishing
#
# Prerequisites: clean, pushed git tree, `gh` signed in, test data present for the world tests
# (RUSTCRAFT_CACHE / RUSTCRAFT_SOLO_WORLD if you want them to really run).
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$DryRun,
    # Hand-written notes, required to publish: what changes for the player, by theme
    # (never automatically generated notes).
    [string]$NotesFile,
    [switch]$SkipTests
)
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)
$tag = "v$Version"
if (-not $DryRun -and (-not $NotesFile -or -not (Test-Path $NotesFile))) {
    throw "release notes missing: -NotesFile <hand-written file>"
}

function Step($msg) { Write-Host "== $msg" -ForegroundColor Cyan }
function Run($exe, [string[]]$a) {
    & $exe @a
    if ($LASTEXITCODE -ne 0) { throw "failed: $exe $($a -join ' ')" }
}

Step "Repository state"
if (git status --porcelain) { throw "git tree not clean: commit or stash first" }
Run git @("fetch", "--quiet", "origin")
$local = git rev-parse HEAD
$remote = git rev-parse "@{u}"
if ($local -ne $remote) { throw "the branch is not in sync with origin: push first" }
if (git tag --list $tag) { throw "the tag $tag already exists" }

# Real release profile (LTO, one unit): not the quick settings used for trials.
Remove-Item Env:CARGO_PROFILE_RELEASE_LTO, Env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS, Env:CARGO_PROFILE_RELEASE_INCREMENTAL, Env:CARGO_PROFILE_RELEASE_DEBUG, Env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER -ErrorAction SilentlyContinue
$env:RUSTCRAFT_LAN_BIND = "127.0.0.1"

Step "Clippy (whole workspace, warnings = errors)"
Run cargo @("clippy", "--release", "--workspace", "--all-targets", "--", "-D", "warnings")

if (-not $SkipTests) {
    Step "Tests (whole workspace)"
    Run cargo @("test", "--release", "--workspace")
}

Step "Release build of the client"
Run cargo @("build", "--release", "-p", "rc-app")

Step "Package"
$out = "target\release-pkg"
$dir = Join-Path $out "rustcraft-$Version-windows-x64"
Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item "target\release\rustcraft.exe" $dir
Copy-Item "LICENSE-MIT", "LICENSE-APACHE" $dir
@"
Rustcraft $Version — Minecraft Java 26.3 client written in Rust (independent project, not affiliated with Mojang).

Installation (you must own Minecraft: Java Edition):
1. In the official launcher, start version 26.3 once, then close it.
2. Put rustcraft.exe in a stable folder (e.g. %LOCALAPPDATA%\rustcraft\bin\).
3. Launcher > Installations > New installation: version release 26.3; More options >
   Java executable > Browse > rustcraft.exe; Create.
4. Play. The first start prepares the version's data (once).

Windows 10/11 64-bit, Vulkan or DX12 graphics card, Microsoft Visual C++ Redistributable (x64).
The executable is not signed: SmartScreen may ask you to "Run anyway".
"@ | Set-Content -Path (Join-Path $dir "INSTALL.txt") -Encoding utf8
$zip = Join-Path $out "rustcraft-$Version-windows-x64.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path "$dir\*" -DestinationPath $zip
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash  $(Split-Path $zip -Leaf)" | Set-Content -Path "$zip.sha256" -Encoding ascii
Write-Host "package: $zip"
Write-Host "sha256: $hash"

if ($DryRun) {
    Step "Dry run: nothing is published"
    exit 0
}

Step "Publishing release $tag"
$notes = @("--notes-file", $NotesFile)
Run gh (@("release", "create", $tag, $zip, "$zip.sha256", "--title", "Rustcraft $Version") + $notes + @("--target", $local))
Write-Host "release published: $tag"
