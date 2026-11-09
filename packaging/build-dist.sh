#!/usr/bin/env bash
# Genera en dist/ todo lo que se sube a una versión de GitHub:
#   pulsa-x86_64-linux    programa para PC (estático: cualquier distro)
#   pulsa-aarch64-linux   lo mismo para ARM64 (Raspberry Pi 3/4/5 con sistema de 64 bits)
#   Pulsa-x86_64.AppImage  con ventana nativa (GTK4 + libadwaita)
#   Pulsa-aarch64.AppImage sin ventana: página de estado en el navegador
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
# AppImage x86_64: con la ventana (GTK4 + libadwaita), compilada en un
# contenedor de Ubuntu 24.04 para que arranque en casi cualquier distro, con
# GTK y libadwaita dentro (packaging/bundle-libs.sh). Requiere podman o docker.
ENGINE=$(command -v podman || command -v docker || true)
IMAGE=pulsa-builder:24.04
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
if [[ -n "$ENGINE" ]]; then
  echo "→ AppImage x86_64 (con ventana)"
  if ! "$ENGINE" image inspect "$IMAGE" >/dev/null 2>&1; then
    "$ENGINE" build -t "$IMAGE" -f - "$ROOT/packaging" <<'EOD'
FROM docker.io/library/ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        curl ca-certificates build-essential pkg-config libgtk-4-dev libadwaita-1-dev file desktop-file-utils patchelf \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal
RUN curl -fsSL -o /usr/local/bin/appimagetool \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage \
    && chmod +x /usr/local/bin/appimagetool
ENV PATH=/root/.cargo/bin:$PATH APPIMAGE_EXTRACT_AND_RUN=1
EOD
  fi
  "$ENGINE" run --rm -v "$ROOT:/src:Z" -w /src -e VERSION="$VERSION" "$IMAGE" bash -euc '
    CARGO_TARGET_DIR=/src/target/appimage cargo build --release --locked --features gui
    APPDIR=$(mktemp -d)/Pulsa.AppDir
    install -Dm755 target/appimage/release/pulsa "$APPDIR/usr/bin/pulsa"
    packaging/bundle-libs.sh "$APPDIR"
    install -Dm755 packaging/AppRun-gui "$APPDIR/AppRun"
    install -Dm644 LICENSE "$APPDIR/usr/share/doc/pulsa/LICENSE"
    install -Dm644 packaging/pulsa.svg "$APPDIR/usr/share/icons/hicolor/scalable/apps/dev.pulsa.Pulsa.svg"
    install -Dm644 packaging/pulsa-256.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/dev.pulsa.Pulsa.png"
    cp packaging/pulsa.svg "$APPDIR/dev.pulsa.Pulsa.svg"
    ln -s dev.pulsa.Pulsa.svg "$APPDIR/.DirIcon"
    sed "s/^Icon=.*/Icon=dev.pulsa.Pulsa/; \$a X-AppImage-Version=$VERSION" packaging/pulsa.desktop > "$APPDIR/dev.pulsa.Pulsa.desktop"
    desktop-file-validate "$APPDIR/dev.pulsa.Pulsa.desktop"
    ARCH=x86_64 appimagetool --no-appstream "$APPDIR" dist/Pulsa-x86_64.AppImage >/dev/null
    chown "$(stat -c %u:%g /src)" dist/Pulsa-x86_64.AppImage
  '
else
  echo "  (sin podman ni docker: no se crea la AppImage x86_64 con ventana)"
fi

# AppImage ARM64 (Raspberry Pi): sin ventana, el binario estático y la página
# de estado en el navegador.
TOOL="$ROOT/target/appimagetool-x86_64.AppImage"
if [[ ! -x "$TOOL" ]]; then
  curl -fL --progress-bar -o "$TOOL" \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"
  chmod +x "$TOOL"
fi
echo "→ AppImage aarch64"
APPDIR="$ROOT/target/AppDir-aarch64"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/scalable/apps"
install -m 755 "$DIST/pulsa-aarch64-linux" "$APPDIR/usr/bin/pulsa"
install -m 755 "$ROOT/packaging/AppRun" "$APPDIR/AppRun"
install -m 644 "$ROOT/packaging/pulsa.svg" "$APPDIR/pulsa.svg"
install -m 644 "$ROOT/packaging/pulsa.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/pulsa.svg"
sed "\$a X-AppImage-Version=$VERSION" "$ROOT/packaging/pulsa.desktop" > "$APPDIR/pulsa.desktop"
cp "$APPDIR/pulsa.desktop" "$APPDIR/usr/share/applications/"
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=aarch64 "$TOOL" --no-appstream "$APPDIR" "$DIST/Pulsa-aarch64.AppImage" >/dev/null 2>&1 \
  || echo "  (no se pudo crear la AppImage aarch64)"

echo "→ apk"
"$ROOT/android/build-apk.sh" >/dev/null
cp "$ROOT/android/pulsa.apk" "$DIST/"
(cd "$DIST" && sha256sum pulsa-* Pulsa-*.AppImage pulsa.apk > SHA256SUMS)
ls -lh "$DIST"
