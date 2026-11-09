#!/usr/bin/env bash
# Genera en dist/ todo lo que se sube a una versión de GitHub:
#   pulsa-x86_64-linux    programa para PC (estático: cualquier distro)
#   pulsa-aarch64-linux   lo mismo para ARM64 (Raspberry Pi 3/4/5 con sistema de 64 bits)
#   Pulsa-x86_64.AppImage  doble clic: arranca y abre la página de estado
#   Pulsa-aarch64.AppImage lo mismo para ARM64
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
# AppImage: el mismo binario estático + lanzador + icono.
TOOL="$ROOT/target/appimagetool-x86_64.AppImage"
if [[ ! -x "$TOOL" ]]; then
  curl -fL --progress-bar -o "$TOOL" \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"
  chmod +x "$TOOL"
fi
for T in x86_64 aarch64; do
  echo "→ AppImage $T"
  APPDIR="$ROOT/target/AppDir-$T"
  rm -rf "$APPDIR"
  mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/scalable/apps"
  install -m 755 "$DIST/pulsa-$T-linux" "$APPDIR/usr/bin/pulsa"
  install -m 755 "$ROOT/packaging/AppRun" "$APPDIR/AppRun"
  install -m 644 "$ROOT/packaging/pulsa.svg" "$APPDIR/pulsa.svg"
  install -m 644 "$ROOT/packaging/pulsa.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/pulsa.svg"
  sed "\$a X-AppImage-Version=$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)" \
    "$ROOT/packaging/pulsa.desktop" > "$APPDIR/pulsa.desktop"
  cp "$APPDIR/pulsa.desktop" "$APPDIR/usr/share/applications/"
  APPIMAGE_EXTRACT_AND_RUN=1 ARCH=$T "$TOOL" --no-appstream "$APPDIR" "$DIST/Pulsa-$T.AppImage" >/dev/null 2>&1 \
    || echo "  (no se pudo crear la AppImage $T)"
done

echo "→ apk"
"$ROOT/android/build-apk.sh" >/dev/null
cp "$ROOT/android/pulsa.apk" "$DIST/"
(cd "$DIST" && sha256sum pulsa-* Pulsa-*.AppImage pulsa.apk > SHA256SUMS)
ls -lh "$DIST"
