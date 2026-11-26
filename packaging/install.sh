#!/bin/sh
# openRMS Watch installer (Linux, any distribution, any architecture we ship).
#
#   ./install.sh                 ask: desktop or headless, and the web port
#   ./install.sh --headless      system service, settings via the web page
#   ./install.sh --desktop       runs as you, with an app-menu launcher
#   ./install.sh --port 18000    web page port (default 17373)
#   ./install.sh --prefix /opt/openrms-watch   (default /usr/local)
#   ./install.sh --uninstall
#
# Run from the unpacked release folder (it holds bin/openrms-watch, bin/mercury
# and packaging/). Needs sudo for --headless and for installing dependencies.
set -eu

MODE="" PORT="" PREFIX=/usr/local UNINSTALL=0 YES=0
while [ $# -gt 0 ]; do
  case "$1" in
    --headless) MODE=headless ;;
    --desktop) MODE=desktop ;;
    --port) PORT="$2"; shift ;;
    --prefix) PREFIX="$2"; shift ;;
    --uninstall) UNINSTALL=1 ;;
    -y|--yes) YES=1 ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

HERE=$(cd "$(dirname "$0")" && pwd)
[ -d "$HERE/packaging" ] || HERE=$(cd "$HERE/.." && pwd)   # running from packaging/
BINDIR="$PREFIX/bin" LIBDIR="$PREFIX/lib/openrms-watch"
SUDO=""; [ "$(id -u)" -eq 0 ] || SUDO="sudo"

ask() { # ask "question" default
  if [ $YES -eq 1 ] || [ ! -t 0 ]; then echo "$2"; return; fi
  printf '%s [%s]: ' "$1" "$2" >&2; read -r a; echo "${a:-$2}"
}

if [ $UNINSTALL -eq 1 ]; then
  $SUDO systemctl disable --now openrms-watch.service 2>/dev/null || true
  systemctl --user disable --now openrms-watch.service 2>/dev/null || true
  $SUDO rm -f "$BINDIR/openrms-watch" /etc/systemd/system/openrms-watch.service
  $SUDO rm -rf "$LIBDIR"
  rm -f "$HOME/.config/systemd/user/openrms-watch.service" "$HOME/.local/share/applications/openrms-watch.desktop"
  echo "Removed openRMS Watch. Your data is kept in /var/lib/openrms-watch and ~/.config/openrms-watch."
  exit 0
fi

[ -n "$MODE" ] || MODE=$(ask "Install as 'desktop' (runs as you, app-menu launcher) or 'headless' (system service, web page only)?" desktop)
case "$MODE" in desktop|headless) ;; *) echo "choose desktop or headless" >&2; exit 2 ;; esac
[ -n "$PORT" ] || PORT=$(ask "Web page port" 17373)
case "$PORT" in ''|*[!0-9]*) echo "port must be a number" >&2; exit 2 ;; esac

# --- dependencies: SoapySDR and the open driver modules -------------------
have_soapy() { ldconfig -p 2>/dev/null | grep -q 'libSoapySDR\.so'; }
if ! have_soapy; then
  echo "SoapySDR (the library that talks to SDRs) is not installed."
  if command -v apt-get >/dev/null; then CMD="apt-get install -y libsoapysdr0.8 soapysdr-module-all soapysdr-tools libasound2 libpulse0"
  elif command -v dnf >/dev/null; then CMD="dnf install -y SoapySDR soapy-rtlsdr soapy-hackrf soapy-airspy alsa-lib pulseaudio-libs"
  elif command -v pacman >/dev/null; then CMD="pacman -S --needed soapysdr soapyrtlsdr soapyhackrf soapyairspy soapyplutosdr alsa-lib libpulse"
  elif command -v zypper >/dev/null; then CMD="zypper install -y libSoapySDR0_8 soapy-rtlsdr soapy-hackrf soapy-airspy alsa libpulse0"
  elif command -v apk >/dev/null; then CMD="apk add soapy-sdr soapy-sdr-rtlsdr soapy-sdr-hackrf alsa-lib libpulse"
  else CMD=""; fi
  if [ -n "$CMD" ] && [ "$(ask "Install it now with: $CMD" yes)" = yes ]; then
    $SUDO sh -c "$CMD" || echo "warning: dependency install failed; install SoapySDR and your SDR's module yourself" >&2
  else
    echo "Install SoapySDR and the module for your SDR, then run openrms-watch devices to check." >&2
  fi
fi

# --- files ------------------------------------------------------------------
$SUDO install -D -m 755 "$HERE/bin/openrms-watch" "$BINDIR/openrms-watch"
$SUDO install -D -m 755 "$HERE/bin/mercury" "$LIBDIR/mercury"
$SUDO install -D -m 644 "$HERE/packaging/mercury-SOURCE.txt" "$LIBDIR/mercury-SOURCE.txt" 2>/dev/null || true

subst() { sed -e "s|@BINDIR@|$BINDIR|g" -e "s|@LIBDIR@|$LIBDIR|g" -e "s|@PORT@|$PORT|g" "$1"; }

if [ "$MODE" = headless ]; then
  getent group openrms-watch >/dev/null || $SUDO groupadd --system openrms-watch
  getent group plugdev >/dev/null || $SUDO groupadd --system plugdev
  id openrms-watch >/dev/null 2>&1 || $SUDO useradd --system --gid openrms-watch --groups plugdev \
      --home-dir /var/lib/openrms-watch --shell /usr/sbin/nologin openrms-watch
  $SUDO install -d -o openrms-watch -g openrms-watch -m 750 /var/lib/openrms-watch
  if [ ! -f /var/lib/openrms-watch/watch.toml ]; then
    subst "$HERE/packaging/watch.toml.default" | $SUDO install -o openrms-watch -g openrms-watch -m 600 /dev/stdin /var/lib/openrms-watch/watch.toml
  fi
  subst "$HERE/packaging/systemd/openrms-watch.service" | $SUDO install -m 644 /dev/stdin /etc/systemd/system/openrms-watch.service
  $SUDO systemctl daemon-reload
  $SUDO systemctl enable --now openrms-watch.service
  URL="http://127.0.0.1:$PORT/"
  echo
  echo "openRMS Watch is running as a service: $URL"
  echo "From another computer: run 'sudo -u openrms-watch $BINDIR/openrms-watch -c /var/lib/openrms-watch/watch.toml set-password YOURPASS',"
  echo "then enable 'Allow other computers' in Settings (or use an SSH tunnel: ssh -L $PORT:127.0.0.1:$PORT this-host)."
  echo "Logs: journalctl -u openrms-watch -f"
else
  CONF="$HOME/.config/openrms-watch"
  mkdir -p "$CONF" "$HOME/.config/systemd/user" "$HOME/.local/share/applications"
  [ -f "$CONF/watch.toml" ] || { subst "$HERE/packaging/watch.toml.default" > "$CONF/watch.toml"; chmod 600 "$CONF/watch.toml"; }
  subst "$HERE/packaging/systemd/user/openrms-watch.service" > "$HOME/.config/systemd/user/openrms-watch.service"
  subst "$HERE/packaging/desktop/openrms-watch.desktop" > "$HOME/.local/share/applications/openrms-watch.desktop"
  if ! id -nG | grep -qw plugdev && getent group plugdev >/dev/null; then
    echo "note: add yourself to 'plugdev' for USB SDR access if your distro uses it: sudo usermod -aG plugdev $USER (log out and in)"
  fi
  systemctl --user daemon-reload
  systemctl --user enable --now openrms-watch.service
  echo
  echo "openRMS Watch is running: http://127.0.0.1:$PORT/  (also 'openRMS Watch' in your app menu)"
  echo "Logs: journalctl --user -u openrms-watch -f"
fi
echo "Next: open the page, go to Settings, pick your SDR and bands."
