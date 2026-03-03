# Handoff Notes — Nimbus (2026-03-03)

Session goal: build a Rust-native KDE-style RSS/Atom reader + CLI, iterate UX, remove tray complexity, and release.

## Current State

- Binary/package name: `nimbus`
- Version in `Cargo.toml`: `0.1.0` (public tag pushed as `v0.1.1`)
- Tray integration has been removed from code and docs.
- App is running as a normal GUI window only.

## Last Commit + Tag

- Commit: `7ca2073`
- Message: `chore(gui): remove tray integration`
- Tag: `v0.1.1` pushed to origin

## What was changed in this cycle

- Removed all tray-specific code paths and args:
  - `src/gui.rs`: removed tray state/methods and `tray_icon`/GTK usage
  - `src/main.rs`: simplified GUI launch (no tray/no-x11 fallback logic)
  - `src/cli.rs`: removed `--no-tray` option
- Updated dependencies:
  - `Cargo.toml`: removed `tray-icon` and `gtk`, left GUI feature as `gui = ["rfd"]`
- Updated docs:
  - `README.md`: removed tray dependency/runtime instructions and `--no-tray` references
  - `CHANGELOG.md`: added Unreleased note about tray removal
- `Cargo.lock` updated by dependency edits

## Build status

- `cargo check --release` completes successfully.
- Current compiler warnings (non-fatal):
  - unused `Context` import in `src/gui.rs`
  - unused storage methods: `upsert_entry`, `latest_readable_timestamp`, `count_feeds` (informational only for now)

## User preferences / UX decisions already applied

- Keep both CLI + GUI in one package.
- No feed read-state cleanup policy (KISS).
- Auto-sync is still periodic.
- Import/Export backup is available via GUI controls.
- “Open” button retained; article link in summary also clickable as needed.
- Feed discovery/dedup behavior remains in place for entered site/feed URLs.

## Pending/next work (if continuing)

1. Ubuntu packaging pass (user wants this eventually):
   - likely local `.desktop` + optional icon install or full `debian/` packaging.
2. Optional code cleanup:
   - remove/resolve current warnings
   - review `src/feed_parser.rs`/`src/storage.rs` for dead-code cleanup
3. Release hardening:
   - decide whether `Cargo.toml` version should bump to `0.1.1` (currently still `0.1.0`)
4. If tray reintroduction becomes desired, design a Wayland-friendly alternative or remove expectation entirely.

### Ubuntu packaging checklist (ready-to-run pass)

1. Install packaging tooling:
   - `cargo install cargo-deb`
2. Add app assets and metadata to repo (recommended paths):
   - `assets/nimbus.desktop`
   - `assets/icons/nimbus.svg` (or a PNG set under hicolor sizes)
3. Add `package.metadata.deb` in `Cargo.toml`:
   - `name`, `maintainer`, `license-file`/`copyright`
   - `depends` (runtime system libs needed by `eframe`)
   - `assets` mapping for binary + desktop file + icon locations
4. Build package:
   - `cargo deb`
5. Install/test locally:
   - `sudo dpkg -i target/debian/nimbus_<version>_amd64.deb`
   - `nimbus` launches GUI from Applications menu
   - `update-desktop-database ~/.local/share/applications` if using a user install path
6. Verify package quality:
   - run `lintian target/debian/nimbus_<version>_amd64.deb`
7. Optional distro-grade step:
   - add Debian directory (`debian/` + `debian/changelog`, `debian/control`, `debian/rules`) for full PPA-style workflow.

## Useful commands

- Run GUI: `cargo run --release`
- Manual sync: `cargo run --release -- sync`
- List unread: `cargo run --release -- list --unread-only --limit 50`
- Export feeds: GUI button or CLI `nimbus export`
- Import feeds: GUI button or CLI `nimbus import <file>`
