#!/usr/bin/env bash
set -euo pipefail

# Local maintenance checks for Tomato Novel Downloader.
# Do not use `cargo --all-features`: `official-api` and `no-official-api`
# are mutually exclusive by design.

skip_fmt=0
skip_no_official=0
skip_tree=0

for arg in "$@"; do
  case "$arg" in
    --skip-fmt) skip_fmt=1 ;;
    --skip-no-official) skip_no_official=1 ;;
    --skip-tree) skip_tree=1 ;;
    -h|--help)
      cat <<'EOF'
Usage: ./scripts/maintain.sh [--skip-fmt] [--skip-no-official] [--skip-tree]

Runs format, test, clippy, and duplicate dependency checks using valid feature combinations.
EOF
      exit 0
      ;;
    *)
      echo "unknown option: $arg" >&2
      exit 2
      ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

step() {
  printf '\n==> %s\n' "$1"
}

step "Rust toolchain"
rustc --version
cargo --version

if [[ "$skip_fmt" -eq 0 ]]; then
  step "Format check"
  cargo fmt --all -- --check
fi

step "Default feature tests"
cargo test

step "Default feature clippy"
cargo clippy --all-targets -- -D warnings

if [[ "$skip_no_official" -eq 0 ]]; then
  step "no-official-api tests"
  cargo test --no-default-features --features no-official-api

  step "no-official-api clippy"
  cargo clippy --no-default-features --features no-official-api --all-targets -- -D warnings
fi

if [[ "$skip_tree" -eq 0 ]]; then
  step "Duplicate dependency overview"
  cargo tree -d
fi

printf '\nAll requested maintenance checks completed.\n'
