#!/usr/bin/env bash
set -euo pipefail

args=("--server")

if [ -n "${TOMATO_DATA_DIR:-}" ]; then
  args+=("--data-dir" "${TOMATO_DATA_DIR}")
fi

if [ -n "${TOMATO_WEB_PASSWORD:-}" ]; then
  args+=("--password" "${TOMATO_WEB_PASSWORD}")
fi

exec /app/tomato-novel-downloader "${args[@]}" "$@"
