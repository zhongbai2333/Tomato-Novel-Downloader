#!/usr/bin/env bash
set -euo pipefail

args=()

has_server=false
for arg in "$@"; do
  if [ "$arg" = "--server" ]; then
    has_server=true
    break
  fi
done

if [ "$has_server" = false ]; then
  args+=("--server")
fi

if [ -n "${TOMATO_DATA_DIR:-}" ]; then
  args+=("--data-dir" "${TOMATO_DATA_DIR}")
fi

if [ -n "${TOMATO_WEB_PASSWORD:-}" ]; then
  args+=("--password" "${TOMATO_WEB_PASSWORD}")
fi

exec /app/tomato-novel-downloader "${args[@]}" "$@"
