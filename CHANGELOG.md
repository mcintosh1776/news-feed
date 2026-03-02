# Changelog

All notable changes to this project are listed here.

## [Unreleased]

- Initial polish pass in progress.

## [0.1.0] - 2026-03-02

### Added

- Desktop GUI (`Nimbus`) with KDE-native look and feel via `eframe/egui`.
- Background sync loop for automatic feed polling.
- Manual CLI interface (`nimbus`) for discover, sync, list, read, import/export, and daemon modes.
- SQLite-backed local data store for feeds and article state.
- Feed discovery from website URLs with normalization and deduping.
- Feed list and article rendering in the GUI with clickable feed/site links.
- Configurable unread-only filtering and article actions (open / mark read / mark unread).
- Import/export of site/feed lists for quick setup transfer.
- Tray integration with unread count.
- Default seed feeds for quick startup:
  - `https://atlas21.com/feed/`
  - `https://www.theblock.co/`

### Changed

- Reworked feed list and article visuals for denser, brighter presentation.
- Added a grabbable feed panel divider/resize handle in the GUI.
- Removed redundant search/filter UI in the feed panel and streamlined controls.

### Fixed

- Feed URL duplicate handling on discovery/import.
- Added defensive handling for invalid feed/title fields to prevent DB insert crashes.
- Adjusted layout behavior so the feed cards area remains fully scrollable and accessible.
