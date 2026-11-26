---
title: Troubleshooting
description: The usual problems and how to fix them
order: 6
---

# Troubleshooting

Start with the **Status** page and the log (`journalctl -u openrms-watch -f` on a headless
install, `journalctl --user -u openrms-watch -f` for a desktop install).

## "No SDR found"

- Check SoapySDR sees it: `SoapySDRUtil --find`. If it lists nothing, the radio's SoapySDR
  module is missing: install `soapysdr-module-all` (Debian/Ubuntu) or your distribution's
  equivalent (`soapy-rtlsdr`, `soapy-hackrf`, ...).
- Check permissions: `SoapySDRUtil --find` working as root but not as you means a udev rule is
  missing or you're not in `plugdev`. Install your radio's package (`rtl-sdr`, `hackrf`), add
  yourself with `sudo usermod -aG plugdev $USER`, and log out and back in. A headless install's
  service user is already in `plugdev`.
- Only one program can use an SDR at a time. Close GQRX, SDR++, GNU Radio and the like.
- RTL-SDR: if the DVB-T kernel driver grabbed it, blacklist `dvb_usb_rtl28xxu`.

## Nothing is ever decoded

Almost always the antenna. Check the **Spectrum** on the Status page: you should see signals
above the grey floor, and FT8 around 7.074 / 14.074 MHz most of the day. If the trace is flat,
see [Choosing an SDR and antenna](hardware.md).

Also check:

- **Bands.** Mercury RMS stations need to be in the segments you're scanning.
- **Decoders all busy.** If every decoder shows *listening* on frequencies that never decode,
  local interference is triggering them. They're released after 45 s and held off for longer
  each time. Move the SDR away from the computer.
- **Activity.** Mercury is newer than VARA or ARDOP and there are fewer stations. Quiet hours
  are normal.

## The SDR clips or is "close to full scale"

Lower the gain (amp first). If it stays high with low gain, a strong local station is
overloading the radio: add an HF low-pass or broadcast-band filter.

## Stations have no name, only a CRC

The station isn't in Atlas's gateway list, or several listed callsigns share that checksum. Set
the **Atlas URL** in Settings (needed for the list even when not contributing). The Status page
shows how many callsigns the watcher has. See [Why some stations have no name](settings.md#why-some-stations-have-no-name).

## "token rejected"

The watcher's key was revoked or replaced (linking again replaces it). Link again from
Settings, or paste a new key. "Watcher disabled by a moderator" means just that; ask the site
operators.

## "Link to openRMS" fails

- "set the Atlas URL first": fill in **Atlas URL**, save, then link.
- "couldn't reach openRMS Accounts": check the internet connection, and **Accounts URL** if you
  changed it (leave it empty for the default).
- The code expires after 15 minutes; press Link again for a new one.

## The page doesn't open

- Wrong port? It's in the log line `web page on http://...`.
- Another program on 17373? Start with `--port 18000`, or change it in Settings.
- From another computer: see [Reaching a headless watcher](install.md#reaching-a-headless-watcher-from-another-computer).
- "another openrms-watch is already running": two copies are pointed at the same settings.
  Stop one.

## Mercury keeps restarting

The log shows `mercury closed the control port` repeatedly. Check the decoder's own log in the
runtime folder (`$XDG_RUNTIME_DIR/openrms-watch/.../mercury.log`). A watcher built from source
needs the patched Mercury from `scripts/build-mercury.sh`, not a stock one.

## Reporting a bug

Open an issue at https://github.com/open-rms/openRMS-Watch/issues with the version
(`openrms-watch --version`), your SDR, your distribution, and the log around the problem.
Remove any watcher key (`ormsw_...`) before pasting a config.
