#!/usr/bin/env bash
# Genera en dist/ todo lo que se sube a una versión de GitHub:
#   pulsa-x86_64-linux    programa para PC (estático: cualquier distro)
#   pulsa-aarch64-linux   lo mismo para ARM64 (Raspberry Pi 3/4/5 con sistema de 64 bits)
#   pulsa.apk             app de Android
#   SHA256SUMS
# Requisitos: rustup con los destinos *-unknown-linux-musl (se añaden solos).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist"
export PATH="$HOME/.cargo/bin:$PATH"

rm -rf "$DIST" && mkdir -p "$DIST"
for T in x86_64 aarch64; do
  rustup target add "$T-unknown-linux-musl" >/dev/null
  echo "→ $T"
  cargo build --release --target "$T-unknown-linux-musl" --manifest-path "$ROOT/Cargo.toml"
  cp "$ROOT/target/$T-unknown-linux-musl/release/pulsa" "$DIST/pulsa-$T-linux"
done
echo "→ apk"
"$ROOT/android/build-apk.sh" >/dev/null
cp "$ROOT/android/pulsa.apk" "$DIST/"
(cd "$DIST" && sha256sum pulsa-* pulsa.apk > SHA256SUMS)
ls -lh "$DIST"
