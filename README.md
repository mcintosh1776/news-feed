# Nimbus

A Rust feed reader inspired by FreshRSS workflow patterns, but fully standalone and local.

This is a native Rust feed engine in one package:

- `Nimbus` (GUI) for KDE desktop monitoring
- `nimbus <command>` (CLI) for automation and agent use

## Features

- Add RSS/Atom feeds directly.
- Discover feed endpoints from website URLs.
- SQLite local store (unread tracking and read state).
- Starts with default seed feeds for testing:
  - `https://atlas21.com/feed/`
  - `https://www.theblock.co/` (auto-discovered to a feed endpoint on first sync)
- Manual `sync` and periodic sync loop (`daemon`).
- KDE-ready GUI with feed list, unread filtering, search, and open/read actions.
- KDE-ready GUI with remove/feed sync actions and multi-line feed add for quick bootstrapping.
- JSON output option for machine usage.

## Important: this is not FreshRSS

- No FreshRSS installation is required.
- No PHP stack.
- No external FreshRSS API or DB dependency.
- A local Rust sync/index pipeline that stores everything in a local SQLite DB.

## Build

```bash
cargo build --release
```

### Dependencies

- KDE/Desktop GUI build has no extra Rust-internal dependencies:
  - Install build prerequisites for your distro as usual for Rust GUI apps (`libgtk-3`/Wayland/X11 toolchain as provided by eframe/eframe dependencies).
- CLI-only usage without GUI deps:

  ```bash
  cargo build --no-default-features
  ``` 

  Then run commands with `--help`, `sync`, `list`, etc.

## KDE desktop integration

After building/installing `nimbus` on your system (`cargo install --path .`), add a desktop launcher:

```desktop
[Desktop Entry]
Type=Application
Name=Nimbus
Comment=KDE-native RSS/Atom feed monitor
Exec=nimbus
Icon=internet-news-reader
Terminal=false
Categories=Network;News;Reader;
StartupNotify=true
```

Save this as `~/.local/share/applications/nimbus.desktop` and then refresh with:

```bash
update-desktop-database ~/.local/share/applications
```

## CLI usage

```bash
# from the repo (if you haven't installed the binary yet)
cargo run --release -- discover https://example.com

# install binary for direct usage
cargo install --path .

# discover feeds from a site
nimbus discover https://example.com

# list configured feeds
nimbus feeds

# sync now
nimbus sync

# list unread entries
nimbus list --unread-only

# search entries and output json
nimbus list --json --limit 20
nimbus search "kde"

# backup site list to restore on another machine
nimbus export > /tmp/nimbus-feeds.txt

# restore site list
nimbus import /tmp/nimbus-feeds.txt

# GUI: export/import in Nimbus GUI via the left panel buttons
- **Export sites...** writes the current site list to a file.
- **Import sites...** loads one site/feed URL per line and adds the feeds.

# mark entry read
nimbus read 12

# run periodic sync daemon
nimbus daemon
```

If you run `nimbus` directly and get `command not found`, use one of the `cargo run -- ...` forms above, or install with `cargo install --path .`.

### GUI quick actions
- Use the left panel to select a feed, `sync` it, or `remove` it.
- You can add several feed URLs at once by pasting lines, commas, or semicolons into the add box.

## GUI

Run without command (or run with `cargo` until installed):

```bash
cargo run --release
```

Default sync interval: 15 minutes (`--interval-minutes`).

## Notes

- Public feeds only in this version.
- Feed parsing is lightweight and handles common RSS/Atom styles.

## Recent UI/UX notes

- Feed panel now uses a draggable width handle (hover turns light so it matches the article divider behavior).
- Feed discovery avoids duplicate URLs and automatically prefers the canonical feed URL.
- Feed URLs and source URLs are clickable directly in the feed list.
