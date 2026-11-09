#!/bin/sh
# Instala Pulsa en este PC (Linux x86_64 o ARM64, como una Raspberry Pi):
#
#   curl -fsSL https://raw.githubusercontent.com/sthifen/pulsa/master/packaging/install.sh | sh
#
# · Descarga el programa a ~/.local/bin/pulsa (un solo archivo, sin dependencias).
# · Lo deja arrancando solo al iniciar sesión (servicio de usuario de systemd).
# · Da permiso a /dev/uinput para el teclado virtual (pide la contraseña una vez;
#   sin eso, en GNOME funciona igual por el escritorio remoto de Mutter).
#
# Desinstalar:  sh install.sh --desinstalar
set -eu

REPO=sthifen/pulsa
BIN="$HOME/.local/bin/pulsa"
UNIT="$HOME/.config/systemd/user/pulsa.service"
RULE=/etc/udev/rules.d/60-pulsa.rules

say() { printf '\033[1;33m→\033[0m %s\n' "$*"; }

if [ "${1:-}" = "--desinstalar" ] || [ "${1:-}" = "--uninstall" ]; then
  systemctl --user disable --now pulsa.service 2>/dev/null || true
  rm -f "$BIN" "$UNIT"
  say "Pulsa desinstalado. (La regla $RULE, si la creaste, se quita con: sudo rm $RULE)"
  exit 0
fi

case "$(uname -m)" in
  x86_64 | amd64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=aarch64 ;;
  *) echo "Pulsa no tiene versión para $(uname -m) todavía. Compílalo con: cargo install --git https://github.com/$REPO" >&2; exit 1 ;;
esac

say "Descargando Pulsa para $ARCH…"
mkdir -p "$(dirname "$BIN")"
TMP="$(mktemp)"
curl -fL --progress-bar "https://github.com/$REPO/releases/latest/download/pulsa-$ARCH-linux" -o "$TMP"
chmod 755 "$TMP"
mv "$TMP" "$BIN"

# Permiso para crear el teclado y el ratón virtuales.
if [ ! -w /dev/uinput ]; then
  say "Dando permiso a /dev/uinput (teclado virtual); se pedirá tu contraseña:"
  if echo 'KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess", OPTIONS+="static_node=uinput"' \
      | sudo tee "$RULE" >/dev/null && sudo udevadm control --reload && sudo udevadm trigger --name-match=uinput; then
    :
  else
    say "Sin permiso: en GNOME se usará el escritorio remoto; en otros escritorios crea $RULE a mano."
  fi
fi

# Arranque automático con la sesión.
if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
  mkdir -p "$(dirname "$UNIT")"
  cat > "$UNIT" <<UNIT
[Unit]
Description=Pulsa: la tablet como teclado y trackpad
After=graphical-session.target

[Service]
ExecStart=$BIN
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
UNIT
  systemctl --user daemon-reload
  systemctl --user enable --now pulsa.service
  say "Pulsa ya está en marcha y arrancará solo al iniciar sesión."
else
  say "Arráncalo con: $BIN"
fi

command -v adb >/dev/null 2>&1 || say "Para usarlo por cable USB instala adb (android-tools). Por WiFi no hace falta."
say "Ahora instala la app en la tablet: https://github.com/$REPO/releases/latest/download/pulsa.apk"
