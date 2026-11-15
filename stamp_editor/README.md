# stamp_editor (crate)

Rust/egui-Implementierung. **Projektübersicht, Release & KI-Hinweis:** siehe [../README.md](../README.md).

## Start

```powershell
cargo run --release
# optional: Pack direkt öffnen
cargo run --release -- --open "..\..\Documents\Anno 117 - Pax Romana\stamps\Region Roman\latium.json"
# legacy Zisternen-Pos in allen Packs unter Ordner korrigieren + speichern
cargo run --release -- --fix-cisterns "..\..\Documents\Anno 117 - Pax Romana\stamps"
```

Katalog lokal unter `../_gamedata/editor_catalog.json` (nicht im Repo). Neu bauen:

```powershell
python ..\_tools\rebuild_editor_catalog.py
```

## Tests

```powershell
cargo test --release
```

Fixtures: `testdata/`.

## Module (Kurz)

| Modul | Rolle |
|-------|------|
| `filedb` | FileDB V3 |
| `stamp` / `pack` | Pipette + Pack-JSON |
| `catalog` | GUIDs, Größen, Needs, Tiers |
| `meshgraph` | Straßen / Aquädukte |
| `coverage` / `economy` / `needs` | Analyse |
| `library` | Bibliothek + Mirror |
| `app` | egui UI |
