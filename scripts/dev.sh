#!/usr/bin/env bash
# One command from a fresh checkout to a running editor, on macOS.
#
#   ./run.sh                 install what is missing, build, start MindCut
#   ./run.sh --setup-only    install what is missing, then stop
#   ./run.sh --yes           do not ask before installing
#   ./run.sh --app           the shipping build (slow to compile, fastest to run)
#   ./run.sh -- <args>       anything after -- goes to the editor
#
# What it checks, and installs when it is not there: the Xcode command-line
# tools, Homebrew, FFmpeg 7+ and cmake and pkgconf from Homebrew, Rust at the
# version src/rust-toolchain.toml pins, and - for the Agent pane, and never
# required - Codex. It changes nothing that is already in place. Linux is not
# set up here: the requirements are listed in README.md.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
assume_yes=0
setup_only=0
profile=quick
passthrough=()

while [ $# -gt 0 ]; do
  case "$1" in
    -y|--yes) assume_yes=1 ;;
    --setup-only) setup_only=1 ;;
    --app) profile=app ;;
    -h|--help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
    --) shift; passthrough=("$@"); break ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

say()  { printf '\033[1m==> %s\033[0m\n' "$*"; }
ok()   { printf '  \033[32mok\033[0m  %s\n' "$*"; }
warn() { printf '  \033[33m!!\033[0m  %s\n' "$*"; }
die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# Asks before installing, unless told not to or there is nobody to ask.
confirm() {
  if [ "$assume_yes" = 1 ] || [ ! -t 0 ]; then return 0; fi
  local answer
  read -r -p "  $1 [Y/n] " answer
  case "$answer" in n|N|no|NO) return 1 ;; *) return 0 ;; esac
}

os=$(uname -s)
if [ "$os" != "Darwin" ]; then
  warn "This script sets up macOS. On $os, install FFmpeg 7+ development libraries, cmake and a C++ compiler (see README.md), then run: cd src && cargo run --profile $profile -p concat"
fi

if [ "$os" = "Darwin" ]; then
  say "Xcode command-line tools"
  if xcode-select -p >/dev/null 2>&1; then
    ok "installed"
  else
    warn "not installed: a window will ask to install them. Run this script again when it finishes."
    xcode-select --install || true
    exit 1
  fi

  say "Homebrew"
  if ! command -v brew >/dev/null 2>&1; then
    for candidate in /opt/homebrew/bin/brew /usr/local/bin/brew; do
      [ -x "$candidate" ] && eval "$("$candidate" shellenv)" && break
    done
  fi
  if command -v brew >/dev/null 2>&1; then
    ok "$(brew --prefix)"
  else
    confirm "Homebrew is not installed. Install it?" || die "Homebrew is needed for FFmpeg and cmake"
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    for candidate in /opt/homebrew/bin/brew /usr/local/bin/brew; do
      [ -x "$candidate" ] && eval "$("$candidate" shellenv)" && break
    done
    command -v brew >/dev/null 2>&1 || die "Homebrew installed, but brew is not on the PATH"
  fi

  say "FFmpeg, cmake, pkgconf"
  missing=()
  for formula in ffmpeg cmake pkgconf; do
    if brew list --versions "$formula" >/dev/null 2>&1; then
      ok "$formula $(brew list --versions "$formula" | awk '{print $2}')"
    else
      missing+=("$formula")
    fi
  done
  if [ ${#missing[@]} -gt 0 ]; then
    confirm "Install ${missing[*]} with Homebrew?" || die "${missing[*]} needed"
    brew install "${missing[@]}"
  fi
  # The bindings need FFmpeg 7 or newer.
  major=$(ffmpeg -version 2>/dev/null | sed -n '1s/^ffmpeg version [a-zA-Z]*\([0-9]*\)\..*/\1/p')
  if [ -n "${major:-}" ] && [ "$major" -lt 7 ]; then
    confirm "FFmpeg $major is too old (7 or newer is needed). Upgrade it?" || die "FFmpeg 7+ needed"
    brew upgrade ffmpeg
  fi
fi

say "Rust"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if ! command -v rustup >/dev/null 2>&1; then
  confirm "Rust is not installed. Install it with rustup?" || die "Rust is needed"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
  . "$HOME/.cargo/env"
fi
channel=$(sed -n 's/^channel = "\(.*\)"/\1/p' "$root/src/rust-toolchain.toml")
[ -n "$channel" ] || die "no channel in src/rust-toolchain.toml"
if rustup toolchain list | grep -q "^$channel"; then
  ok "toolchain $channel"
else
  say "Installing Rust $channel (the version this project pins)"
  rustup toolchain install "$channel" --profile minimal -c rustfmt -c clippy
fi

say "Codex (for the Agent pane)"
codex_found=""
for candidate in "$(command -v codex 2>/dev/null || true)" /opt/homebrew/bin/codex /usr/local/bin/codex /Applications/Codex.app/Contents/Resources/codex; do
  if [ -n "$candidate" ] && [ -x "$candidate" ]; then codex_found=$candidate; break; fi
done
if [ -n "$codex_found" ]; then
  ok "$codex_found"
elif command -v npm >/dev/null 2>&1 && confirm "Codex is not installed. Install it with npm (npm i -g @openai/codex)?"; then
  npm install -g @openai/codex || warn "the install failed; the editor works without it, the Agent pane will say Codex is missing"
  warn "sign in once with: codex login"
else
  warn "not installed. The editor works without it; the Agent pane needs it: npm i -g @openai/codex, then codex login"
fi

if [ "$setup_only" = 1 ]; then
  say "Ready. Start it with: ./run.sh"
  exit 0
fi

say "Building and starting MindCut ($profile profile)"
echo "  The first build compiles the whole editor and takes a while; later ones take seconds."
cd "$root/src"
if [ ${#passthrough[@]} -gt 0 ]; then
  exec cargo run --profile "$profile" -p concat -- "${passthrough[@]}"
fi
exec cargo run --profile "$profile" -p concat
