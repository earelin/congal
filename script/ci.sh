#!/usr/bin/env bash
#
# ci.sh — executa en local as mesmas comprobacións que o pipeline de CI
# (.github/workflows/ci.yml). Lánzao antes de facer push.
#
#   ./script/ci.sh
#
# Sae cun código distinto de 0 á primeira comprobación que falle.

set -euo pipefail

# Trata os avisos como erros, igual que en CI.
export RUSTFLAGS="${RUSTFLAGS:-} -D warnings"
export CARGO_TERM_COLOR=always

# Sitúate na raíz do repositorio independentemente de onde se invoque.
cd "$(dirname "$0")/.."

step() { printf '\n\033[1;34m==> %s\033[0m\n' "$1"; }

step "Formato (cargo fmt --check)"
cargo fmt --all -- --check

step "Clippy"
cargo clippy --all-targets --all-features

step "Build (release)"
cargo build --release --locked

step "Tests"
cargo test --release --locked

printf '\n\033[1;32m✓ Todas as comprobacións de CI pasaron\033[0m\n'
