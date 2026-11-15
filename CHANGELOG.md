# Changelog

All notable changes to the Anno 117 Stamp Editor are documented here.

## [0.2.1] — 2026-11-15

### Added

- **First-run setup wizard**: choose game install (icons), game stamps folder, and editor library before anything else runs
- Portable config next to the EXE (or `%LOCALAPPDATA%\Anno117StampEditor`) so a standalone ZIP works without Python
- Menu entry **Datei → Erste Einrichtung…** to re-open the wizard
- README screenshots (setup, Hauptfenster, Menü Datei) + clearer Ubisoft vs. MIT rights split

### Fixed

- No auto-import of Documents stamps / library seed **before** setup is confirmed (empty library until „Fertig“)
- Dimmed modal overlay so the wizard cannot be missed on first launch
- Catalog path resolution for release EXE vs. local `_gamedata` without hardcoding one PC

## [0.2.0] — 2026-11-15

### Added

- Dual library workflow: safe library outside Documents + mirror into game stamps
- Aqueduct editing: MinGround / MaxGround, coalesced mesh spans, cistern inlet hints
- Pipette encode compatible with in-game stamps (zlib footer / modern Variation rules)
- Residence tier tags **T1–T4** on canvas + build strip; tier-tinted footprints
- Zoom-aware coverage chips (dots when zoomed out instead of text spam)
- MaxGround aqueduct tiles drawn in blue family (not beige)
- Y-up canvas mapping for Anno stamp space
- German/English UI strings (`i18n`)

### Fixed

- **Aqueduct cistern +1 tile offset**: BuildBlocker pivot sits 1 tile left of the geometric center (`Pos` for flush TL is `1.5`, not centered `2.5`). Size is forced to 3×3. Known cistern GUIDs (19753 / 29526) always win over catalog `blocker_min`. GUID-swap re-anchors Pos. **Load-time migrate** rewrites legacy centered `Pos` (road-flush heuristic / unmarked → decentered) and marks `RusticusCisternPivot`; open sets dirty so Save persists the fix. CLI: `cargo run -- --fix-cisterns <dir|pack.json>` batch-rewrites packs.
- Ghost roads after naïve Y-reflect (full mesh wipe before rewrite)
- Inverted tile rects after Y-up flip (footprints/roads invisible)
- Cistern `Variation=0` rejected by game — cisterns omit Variation
- Half-tile aqueduct stubs / tower spam — centre-to-centre coalesce + MaxGround over roads
- Half-tile paste offset (clipboard uses footprint top-left deltas)
- PlaceRect grid sized by clipboard group span (not primary footprint alone)
- Delete removes selected roads as well as buildings
- Badge colors: ≥100% always green for seeds and affected peers alike

### Notes

- Game blueprints often store only the T1 residence GUID; higher tiers may not round-trip from in-game stamps.
- `_gamedata/` remains local-only (regenerate from your game install).

## [0.1.0] — initial snapshot

- Stamp pack open/edit/save (FileDB V3 round-trip)
- Building palette, coverage over roads, production-chain panel
- ComplexOwner / farm modules, library, build menu, radial actions
- Repo hygiene: no unpacked game UI / catalogs on GitHub
