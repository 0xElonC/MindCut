<#
.SYNOPSIS
  One command from a fresh checkout to a running editor, on Windows.

.DESCRIPTION
  run.cmd                  install what is missing, build, start MindCut
  run.cmd -SetupOnly       install what is missing, then stop
  run.cmd -Yes             do not ask before installing
  run.cmd -App             the shipping build (slow to compile, fastest to run)

  Checks, and installs with winget when it is not there: the Visual Studio C++
  build tools, Rust at the version src\rust-toolchain.toml pins, cmake, LLVM
  (libclang, for the bindings) and, for the Agent pane and never required,
  Codex. FFmpeg's development libraries are downloaded - the build and checksum
  scripts\ci\deps.lock pins - into src\vendor\ffmpeg\win64. Nothing already in
  place is changed, and nothing is set outside this run: the paths it needs
  are set for this process only.
#>
[CmdletBinding()]
param(
  [switch]$Yes,
  [switch]$SetupOnly,
  [switch]$App,
  [Parameter(ValueFromRemainingArguments = $true)] [string[]]$EditorArgs
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$profileName = 'quick'
if ($App) { $profileName = 'app' }

function Say($text)  { Write-Host "==> $text" -ForegroundColor White }
function Ok($text)   { Write-Host "  ok  $text" -ForegroundColor Green }
function Warn($text) { Write-Host "  !!  $text" -ForegroundColor Yellow }
function Die($text)  { Write-Host "error: $text" -ForegroundColor Red; exit 1 }

# Asks before installing, unless told not to or there is nobody to ask.
function Confirm-Step($question) {
  if ($Yes -or -not [Environment]::UserInteractive) { return $true }
  $answer = Read-Host "  $question [Y/n]"
  return ($answer -notmatch '^(n|no)$')
}

function Have($name) { return [bool](Get-Command $name -ErrorAction SilentlyContinue) }

# A program installed a moment ago is on the machine's PATH, and not yet on
# this process's: read it again.
function Update-Path {
  $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
  $user = [Environment]::GetEnvironmentVariable('Path', 'User')
  $env:Path = "$machine;$user;$env:USERPROFILE\.cargo\bin"
}

function Install-WithWinget($id, $what, $override) {
  if (-not (Have 'winget')) {
    Die "winget is not available, and $what is not installed. Install 'App Installer' from the Microsoft Store (or $what by hand), then run this again."
  }
  $arguments = @('install', '--id', $id, '-e', '--accept-source-agreements', '--accept-package-agreements')
  if ($override) { $arguments += @('--override', $override) }
  & winget @arguments
  # -1978335189: already installed, nothing newer.
  if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) {
    Die "winget could not install $what (exit $LASTEXITCODE)."
  }
  Update-Path
}

if (-not [Environment]::Is64BitOperatingSystem) { Die 'A 64-bit Windows is needed.' }
if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
  Warn 'Windows on ARM is not built or tested by this project; the editor may not start.'
}
Update-Path

# -- Visual Studio C++ build tools ------------------------------------------
Say 'Visual Studio C++ build tools'
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$tools = $null
if (Test-Path $vswhere) {
  $tools = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}
if ($tools) {
  Ok $tools
} else {
  if (-not (Confirm-Step 'The C++ build tools are missing. Install them (a few GB, asks for administrator rights)?')) { Die 'The C++ build tools are needed.' }
  Install-WithWinget 'Microsoft.VisualStudio.2022.BuildTools' 'the Visual Studio C++ build tools' '--passive --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
}

# -- Rust ------------------------------------------------------------------
Say 'Rust'
if (-not (Have 'rustup')) {
  if (-not (Confirm-Step 'Rust is not installed. Install it with rustup?')) { Die 'Rust is needed.' }
  Install-WithWinget 'Rustlang.Rustup' 'Rust (rustup)' $null
  if (-not (Have 'rustup')) { Die 'rustup was installed but is not on the PATH. Open a new terminal and run this again.' }
}
$channelLine = Select-String -Path "$root\src\rust-toolchain.toml" -Pattern '^channel\s*=\s*"(.*)"'
if (-not $channelLine) { Die 'no channel in src\rust-toolchain.toml' }
$channel = $channelLine.Matches[0].Groups[1].Value
$installed = & rustup toolchain list
if ($installed -match "^$([regex]::Escape($channel))") {
  Ok "toolchain $channel"
} else {
  Say "Installing Rust $channel (the version this project pins)"
  & rustup toolchain install $channel --profile minimal -c rustfmt -c clippy
  if ($LASTEXITCODE -ne 0) { Die 'rustup could not install the toolchain.' }
}

# -- cmake -----------------------------------------------------------------
Say 'cmake'
if (-not (Have 'cmake')) {
  if (Test-Path "$env:ProgramFiles\CMake\bin\cmake.exe") {
    $env:Path += ";$env:ProgramFiles\CMake\bin"
  } else {
    if (-not (Confirm-Step 'cmake is not installed. Install it?')) { Die 'cmake is needed.' }
    Install-WithWinget 'Kitware.CMake' 'cmake' $null
    if (-not (Have 'cmake')) { $env:Path += ";$env:ProgramFiles\CMake\bin" }
  }
}
Ok ((& cmake --version | Select-Object -First 1))

# -- LLVM (libclang, for the bindings) -------------------------------------
Say 'LLVM (libclang)'
$libclang = $null
foreach ($dir in @($env:LIBCLANG_PATH, "$env:ProgramFiles\LLVM\bin", 'C:\llvm\bin')) {
  if ($dir -and (Test-Path "$dir\libclang.dll")) { $libclang = $dir; break }
}
if (-not $libclang) {
  if (-not (Confirm-Step 'LLVM is not installed. Install it?')) { Die 'LLVM (libclang) is needed.' }
  Install-WithWinget 'LLVM.LLVM' 'LLVM' $null
  if (Test-Path "$env:ProgramFiles\LLVM\bin\libclang.dll") { $libclang = "$env:ProgramFiles\LLVM\bin" }
}
if (-not $libclang) { Die 'libclang.dll was not found after installing LLVM.' }
$env:LIBCLANG_PATH = $libclang
Ok $libclang

# -- FFmpeg development libraries ------------------------------------------
Say 'FFmpeg development libraries'
$ffmpeg = "$root\src\vendor\ffmpeg\win64"
if (-not (Test-Path "$ffmpeg\include\libavcodec\avcodec.h")) {
  $row = Select-String -Path "$root\scripts\ci\deps.lock" -Pattern '^ffmpeg-win64\s+(\S+)\s+(\S+)'
  if (-not $row) { Die 'deps.lock has no ffmpeg-win64 row.' }
  $url = $row.Matches[0].Groups[1].Value
  $digest = $row.Matches[0].Groups[2].Value
  $zip = Join-Path $env:TEMP 'eloncut-ffmpeg-win64.zip'
  Say "Downloading $url"
  $ProgressPreference = 'SilentlyContinue'
  Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
  $got = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
  if ($got -ne $digest.ToLower()) {
    Remove-Item $zip -ErrorAction SilentlyContinue
    Die "The FFmpeg download does not match the checksum in deps.lock (wanted $digest, got $got)."
  }
  $unpacked = Join-Path $env:TEMP 'eloncut-ffmpeg-unpack'
  if (Test-Path $unpacked) { Remove-Item $unpacked -Recurse -Force }
  Expand-Archive -Path $zip -DestinationPath $unpacked
  $inner = Get-ChildItem $unpacked -Directory | Select-Object -First 1
  if (-not $inner) { Die 'The FFmpeg archive was empty.' }
  New-Item -ItemType Directory -Force -Path (Split-Path $ffmpeg) | Out-Null
  if (Test-Path $ffmpeg) { Remove-Item $ffmpeg -Recurse -Force }
  Move-Item $inner.FullName $ffmpeg
  Remove-Item $zip, $unpacked -Recurse -Force -ErrorAction SilentlyContinue
}
$env:FFMPEG_DIR = $ffmpeg
# The editor loads FFmpeg's DLLs when it starts.
$env:Path = "$ffmpeg\bin;$env:Path"
Ok $ffmpeg

# -- Codex, for the Agent pane ---------------------------------------------
Say 'Codex (for the Agent pane)'
$codex = $null
foreach ($name in @('codex.exe', 'codex.cmd')) {
  $found = Get-Command $name -ErrorAction SilentlyContinue
  if ($found) { $codex = $found.Source; break }
}
if (-not $codex -and (Test-Path "$env:APPDATA\npm\codex.cmd")) { $codex = "$env:APPDATA\npm\codex.cmd" }
if ($codex) {
  Ok $codex
} else {
  $npm = Have 'npm'
  if (-not $npm -and (Confirm-Step 'Codex needs Node.js, which is not installed. Install Node.js LTS?')) {
    Install-WithWinget 'OpenJS.NodeJS.LTS' 'Node.js' $null
    $npm = Have 'npm'
  }
  if ($npm -and (Confirm-Step 'Codex is not installed. Install it with npm (npm i -g @openai/codex)?')) {
    & npm install -g '@openai/codex'
    if ($LASTEXITCODE -ne 0) {
      Warn 'The install failed. The editor works without it; the Agent pane will say Codex is missing.'
    } else {
      Update-Path
      Warn 'Sign in once with: codex login'
    }
  } else {
    Warn 'Not installed. The editor works without it; the Agent pane needs it: npm i -g @openai/codex, then codex login'
  }
}

if ($SetupOnly) {
  Say 'Ready. Start it with: run.cmd'
  exit 0
}

Say "Building and starting MindCut ($profileName profile)"
Write-Host '  The first build compiles the whole editor and takes a while; later ones take seconds.'
Push-Location "$root\src"
try {
  $cargoArguments = @('run', '--profile', $profileName, '-p', 'concat')
  if ($EditorArgs -and $EditorArgs.Count -gt 0) {
    $cargoArguments += '--'
    $cargoArguments += $EditorArgs
  }
  & cargo @cargoArguments
  exit $LASTEXITCODE
} finally {
  Pop-Location
}
