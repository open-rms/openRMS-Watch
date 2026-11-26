---
title: Install
description: Desktop packages, headless installs and Raspberry Pi
order: 1
---

# Install

openRMS Watch runs on Linux: any distribution, on x86_64, ARM (64-bit and 32-bit Raspberry
Pi) and RISC-V. Downloads are on the [releases page](https://github.com/open-rms/openRMS-Watch/releases).

Pick one way to install:

| You want | Download | Then |
|---|---|---|
| A window and a tray icon on your desktop (Debian, Ubuntu, Mint) | `openrms-watch-desktop_<version>_amd64.deb` or `_arm64.deb` | `sudo apt install ./openrms-watch-desktop_*.deb` |
| The same on Fedora or openSUSE | `openrms-watch-desktop-<version>-1.x86_64.rpm` or `.aarch64.rpm` | `sudo dnf install ./openrms-watch-desktop-*.rpm` |
| The same on any other distribution | `openrms-watch-desktop_<version>_amd64.AppImage` | `chmod +x` it and run it |
| A box with no screen, or a Raspberry Pi | `openrms-watch-<version>-linux-<arch>.tar.gz` | `./install.sh --headless` (below) |
| Alpine Linux | `openrms-watch-<version>-linux-<arch>-musl.tar.gz` | `./install.sh --headless` |

Check what you downloaded against `SHA256SUMS` from the same release
(`sha256sum -c --ignore-missing SHA256SUMS`). Builds are not signed yet.

## What every install needs

- **SoapySDR and the module for your radio.** This is the library that talks to SDRs. On
  Debian and Ubuntu: `sudo apt install libsoapysdr0.8 soapysdr-module-all`. The headless
  installer offers to install it for you on apt, dnf, pacman, zypper and apk systems.
- **Permission to use the USB radio.** Most distributions ship udev rules with the radio's
  package that let members of `plugdev` use it. If the watcher can't see your SDR, see
  [Troubleshooting](troubleshooting.md).
- **An HF antenna.** The watcher listens for Mercury on HF. A stock telescopic antenna on a
  HackRF or RTL-SDR will not hear anything useful. See [Choosing an SDR and antenna](hardware.md).

## Desktop app

The desktop app is a window onto the watcher's own web page, plus a tray icon that shows what
it's doing: bands scanned, decoders busy, whether it's contributing, and SDR clipping. From
the tray you can open the window, restart the watcher, turn on **Start at login**, or stop it.
Closing the window keeps the watcher running in the tray.

On first start it writes its settings to `~/.config/openrms-watch/watch.toml` and opens the
page. Go to **Settings**, pick your SDR and the bands to watch. See [Settings and status](settings.md).

## Headless install (servers, Raspberry Pi)

```sh
tar xzf openrms-watch-*-linux-aarch64.tar.gz
cd openrms-watch-*/
./install.sh --headless              # system service, web page on port 17373
./install.sh --headless --port 18000 # pick another port
```

`install.sh` asks which way to install if you don't pass `--headless` or `--desktop`:

- **headless**: a systemd service (`openrms-watch.service`) running as its own user, with its
  settings and data in `/var/lib/openrms-watch`. You change settings from the web page.
- **desktop**: runs as you (a systemd user service) and adds an "openRMS Watch" entry to your
  app menu that opens the page in your browser.

Other options: `--prefix /opt/openrms-watch` (default `/usr/local`), `--yes` (accept the
defaults), `--uninstall` (removes the program and service; your data is kept).

### Reaching a headless watcher from another computer

The web page only listens on the machine itself (`127.0.0.1`) until you allow more. Either:

- use an SSH tunnel: `ssh -L 17373:127.0.0.1:17373 pi@yourpi`, then open
  http://127.0.0.1:17373/ on your own computer; or
- set a password and open the page to your network:
  ```sh
  sudo -u openrms-watch openrms-watch -c /var/lib/openrms-watch/watch.toml set-password 'something long'
  ```
  then tick **Allow other computers** in Settings. The page refuses setting changes from the
  network until a password is set.

### Raspberry Pi sizing

A Raspberry Pi 4 or 5 is comfortable with one RTL-SDR covering one band (2.4 MS/s) and four
decoders. Scanning several bands at once with a HackRF at 16 MS/s wants a desktop-class CPU.
The Settings page shows a CPU estimate for what you choose.

## Ports

| What | Default | Change it with |
|---|---|---|
| Web page | 17373 | `--port N`, `OPENRMS_WATCH_PORT=N`, the installer, or Settings |
| Mercury decoders (localhost only) | picked free from 17400-17999 | `base_port` in `watch.toml` |

## Logs

- headless: `journalctl -u openrms-watch -f`
- desktop via installer: `journalctl --user -u openrms-watch -f`
- the desktop app started on its own: run `openrms-watch-desktop` from a terminal

## Build from source

You need Rust (stable), a C compiler, `pkg-config`, and the SoapySDR, ALSA and PulseAudio
development packages.

```sh
git clone https://github.com/open-rms/openRMS-Watch && cd openRMS-Watch
scripts/build-mercury.sh        # patched Mercury into .build/mercury/
cargo build --release
./target/release/openrms-watch -c examples/watch.toml run
```

`scripts/package.sh` makes a release tarball, `scripts/build-desktop.sh` the desktop
packages, and `scripts/build-in-docker.sh linux/arm64` builds for another architecture.
