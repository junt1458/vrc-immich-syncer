# vrc-immich-syncer

A resident client that watches your VRChat screenshot folders and
automatically uploads new images to an [Immich](https://immich.app/) server.

- **One-way sync** (PC → Immich only): uploads local files missing on the
  server; never downloads or modifies local files.
- **Watch + catch-up scans**: a live folder watcher picks up new screenshots
  as they're written, plus a full scan at startup and on a periodic interval
  so anything missed while the client wasn't running still gets uploaded.
- **Deletion-safe**: if an asset is later deleted on the Immich server, the
  local file is kept but never re-uploaded.
- **Tray + notifications on Windows**: a tray icon reflects sync state
  (idle/syncing/error), with OS toast notifications on upload failure and a
  "recovered" toast once retries succeed.
- **Screenshot metadata → description**: VRChat native
  (`--enable-screenshot-metadata`) and VRCX (Screenshot Helper) PNG metadata
  (world name, players) is parsed and stored as the Immich asset description.

## Platforms

- **Windows** is the primary, fully supported target (tray icon, toast
  notifications, login auto-start via the registry Run key).
- **Linux/macOS**: core functionality (scan / watch / upload) works. The tray
  icon is best-effort: on Linux it requires GTK and is behind the `tray`
  feature (on by default); if tray initialization fails, the app still runs
  with the window alone.

## Build

```sh
cargo build --release
```

The binary is built as `vrc-immich-syncer` (`target/release/vrc-immich-syncer`,
or `.exe` on Windows).

### Linux system dependencies

Building/running the `tray` feature and the eframe window under X11 requires
these system packages (see `.devcontainer/devcontainer.json`):

```sh
sudo apt-get install -y libgtk-3-dev libayatana-appindicator3-dev libxdo-dev libxkbcommon-x11-0
```

To build without the tray icon (no GTK dependency), disable default
features:

```sh
cargo build --release --no-default-features
```

## Usage

Run the app:

```sh
cargo run -p syncer-app
# or, from a release build:
./target/release/vrc-immich-syncer
```

On first launch, open the **Settings** tab and configure:

- **Server URL** and **API key** for your Immich instance (use "Test
  connection" to verify).
- One or more **watch folders** (e.g. your VRChat screenshots directory).
- Optional: auto-add uploads to an Immich album, screenshot
  metadata → description, allowed file extensions, scan/retry intervals, and
  auto-start at login.

Saving applies changes live — watchers restart and the scanner reschedules
without needing to relaunch the app.

Pass `--minimized` to launch with the window hidden and only the tray icon
visible (this is the flag used automatically by the login auto-start entry):

```sh
vrc-immich-syncer.exe --minimized
```

## Development

See [`AGENTS.md`](AGENTS.md) for the full contributor/agent guidance:
workspace layout, testing policy, coding conventions, and commit style.

## License

MIT — see [`LICENSE`](LICENSE).
