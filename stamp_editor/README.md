# stamp_editor (crate)

Rust/egui implementation. **Project overview, release & AI note:** see [../README.md](../README.md).

## Run

```powershell
cargo run --release
# optional: open a pack directly
cargo run --release -- --open "..\..\Documents\Anno 117 - Pax Romana\stamps\Region Roman\latium.json"
# rewrite legacy cistern Pos under a folder and save
cargo run --release -- --fix-cisterns "..\..\Documents\Anno 117 - Pax Romana\stamps"
```

Local catalog at `../_gamedata/editor_catalog.json` (not in the repo). Rebuild:

```powershell
python ..\_tools\rebuild_editor_catalog.py
```

## Tests

```powershell
cargo test --release
```

Fixtures: `testdata/`.

## Modules (short)

| Module | Role |
|--------|------|
| `filedb` | FileDB V3 |
| `stamp` / `pack` | Pipette + pack JSON |
| `catalog` | GUIDs, sizes, needs, tiers |
| `meshgraph` | Roads / aqueducts |
| `coverage` / `economy` / `needs` | Analysis |
| `library` | Library + mirror |
| `app` | egui UI |
