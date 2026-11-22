#!/usr/bin/env bash
# Installs what is missing and starts MindCut. See scripts/dev.sh.
exec "$(cd "$(dirname "$0")" && pwd)/scripts/dev.sh" "$@"
