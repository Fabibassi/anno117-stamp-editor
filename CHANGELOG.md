# Changelog

All notable changes to the Anno 117 Stamp Editor (Rusticus) are documented here.

## [Unreleased]

### Added

- **Resource-area efficiency** for Forest / Meadow / Marsh / Embankment producers (`area_efficiency`): have/need, influence circles, exclusive tile claim + need-priority for contested tiles (Anno-1800-style model; Anno 117 calibration pending in-game checks).
- **Multi-select influence overlays**: circles and have/need badges for all selected resource buildings.
- **Affected peers**: non-selected same-class buildings whose radius overlaps a moved/selected producer also show have/need (amber ring; badge color follows fill ratio).
- **Marquee copy/paste of groups**: buildings + roads; paste places the whole clipboard group (not a single woodcutter); Ctrl+V via OS clipboard marker.
- **SelectRect roads**: roads inside the blue marquee are included in selection/copy.
- **Multi-drag**: moving one selected building moves the whole selection (buildings + roads).
- **Radial RMB cancel**: right-click elsewhere with the radial open cancels instead of reopening.

### Fixed

- **Aqueduct cistern +1 tile offset**: BuildBlocker pivot sits 1 tile left of the geometric center (`Pos` for flush TL is `1.5`, not centered `2.5`). Size is forced to 3×3. Known cistern GUIDs (19753 / 29526) always win over catalog `blocker_min`. GUID-swap re-anchors Pos. **Load-time migrate** rewrites legacy centered `Pos` (road-flush heuristic / unmarked → decentered) and marks `RusticusCisternPivot`; open sets dirty so Save persists the fix. CLI: `cargo run -- --fix-cisterns <dir|pack.json>` batch-rewrites packs.
- Half-tile paste offset (clipboard uses footprint top-left deltas).
- PlaceRect grid sized by clipboard group span (not primary footprint alone).
- Delete removes selected roads as well as buildings.
- Badge colors: ≥100% always green for seeds and affected peers alike.

### Notes / open

- Contested-forest assignment still based on the Anno 1800 exclusive model. In-game Anno 117 blueprint comparison may change this.
- Farm field free-paint, aqueduct/drainage edit tools, library DnD polish remain open (see README).

## [0.1.0] — initial snapshot

- Stamp pack open/edit/save (FileDB V3 round-trip).
- Building palette, coverage over roads, production-chain panel.
- ComplexOwner / farm modules, library, build menu, radial actions.

### Repo hygiene

- No unpacked game UI (`ui_png`), no `assets.xml`, no derived `_gamedata` catalogs on GitHub — regenerate locally.
