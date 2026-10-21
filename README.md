# Gluco

A Linux desktop window for the glucose reading on a [LibreLinkUp](https://www.librelinkup.com/) account. It stays open, refreshes about once a minute, and lays the current value out in the same spirit as the FreeStyle Libre phone screen: the number, the trend arrow, the shaded range, recent history, and how long the sensor has left.

Gluco is not made by Abbott. It is not a medical device. Readings from LibreLinkUp can be delayed or missing. Dose and treat from the FreeStyle Libre app or reader.

## Install

### Debian, Ubuntu, and derivatives

Build a `.deb` on the machine that will run it (or any machine of the same architecture), then install that package:

```bash
bash packaging/build-deb.sh
sudo apt install ./dist/gluco_*_*.deb
```

The package puts `gluco` on `PATH`, adds a menu entry, and depends on the shared libraries the window needs. `fonts-inter` is recommended so the window uses the face it was drawn with.

`dist/gluco_*_amd64.deb` is a prebuilt package for x86-64. It needs glibc 2.39 or newer, which means Ubuntu 24.04 and Debian 13. On an older release, run `bash packaging/build-deb.sh` there and install the package that command writes.

### Fedora

```bash
bash packaging/fedora/install.sh
```

The script installs the compiler and GUI libraries with `dnf`, builds the release binary, and installs it under `/usr/local`. It asks for `sudo` only for those system changes. Remove it later with `bash packaging/fedora/install.sh --uninstall`. A checkout from GitHub does not keep the executable bit on these scripts, so run them with `bash`.

## Run from source

Install Rust (1.92 or newer) and the usual Linux GUI libraries:

```bash
sudo apt install libfontconfig1 libxkbcommon0 libxcb1 libgl1
cargo run --release
```

Sign in with the **LibreLinkUp** email and password (the follower app), not a different Abbott login. If the account follows more than one person, pick who to show on the Profile tab.

Leave **Remember on this computer** checked to store the sign-in in your config folder, mode `0600`, typically `~/.config/gluco/session.json`. The path is shown on the Profile tab. Uncheck it to keep the password out of that file.

`Automatic` region follows LibreLinkUp’s redirect. If sign-in cannot find the account, choose the region that matches the app.

The window refreshes every 60 seconds. Faster polling is intentionally not offered: LibreLinkUp locks sign-in after repeated attempts.

## What you can do

- Home shows the current value, trend, a 3 / 6 / 12 / 24 hour chart, and sensor time remaining.
- Insights summarizes the readings in the selected window: average, lowest, highest, and time in range.
- Alarms lists the thresholds that came back with the account. Optional desktop notices fire when a reading leaves the range. They are easy to miss and can lag. The phone alarms are the ones that matter.
- Profile switches the person, chooses 14 or 15 day sensor wear for the countdown, pins the window above others, refreshes, and signs out.

**Preview with sample data** on the sign-in screen draws a fake trace so the layout can be checked without an account. A banner marks it as sample data.

Libre 3 Plus and Libre 2 Plus wear for 15 days, so that is the default countdown for a Libre 3 connection. Older sensors wear for 14 days. Switch it on the Profile tab if the countdown is a day off. It does not affect the glucose value.

## If sign-in fails

- Wrong email or password: wait before trying again. Several failures lock the follower sign-in even when the phone app still opens.
- “Nobody is sharing a sensor”: in the FreeStyle Libre app, invite this LibreLinkUp email, accept the invite in LibreLinkUp, then refresh.
- A protected or refused response: Abbott has been replacing the JSON follower responses with an encrypted one. Gluco does not bypass that. Use the LibreLinkUp app when it happens.
- A blank window or a graphics error: `SLINT_RENDERER=software cargo run --release`.

## Develop

```bash
cargo test
cargo run
GLUCO_PREVIEW=1 cargo run
```

The UI is [Slint](https://slint.dev/). Slint is used under its own license (`GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`). The rest of this project is MIT.
