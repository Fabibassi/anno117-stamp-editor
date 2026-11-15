# Anno 117 Stamp Editor (Rust / egui)

Desktop-GUI zum **Öffnen, Bearbeiten und Speichern** von Anno-117-Stamp-Packs – inkl. Coverage über Wege und Produktionsketten-Rechner.

## Features

- Stamp-Pack JSON per **Drag & Drop** oder Datei-Dialog öffnen
- **Gebäude-Palette** (Baulette / Filter) aus Asset-Katalog
- Gebäude **verschieben** (LMB Drag, 0.5-Raster)
- Gebäude **platzieren** (Palette wählen → leere Zelle LMB)
- **GUID tauschen** (Palette wählen → RMB auf Gebäude)
- Speichern zurück in dieselbe Pack-JSON (Autosave)
- **Coverage / Radius** bei Auswahl (InfluenceRadius, Ausbreitung über Dreck-/Steinwege)
- **Layout-Effizienz**: BBox, leere Zellen, Fill-Ratio, Wege-Zählung
- **Flächen-Effizienz** (Holzfäller & Co.): Influence-Kreise, have/need, Konkurrenz gleicher Klasse
- **Multi-Select / Copy-Paste**: Marquee inkl. Wege, Gruppen-Ghost, Multi-Drag, Ctrl+V
- **Produktionskette**: Ressourcen rein/raus pro Minute, Defizit-Markierung
- **ComplexOwner / Farmen**: Felder & Module, Outline, have/need, Modul platzieren

## Start

```powershell
cd "C:\Users\fabi_\Projects\anno117-stamp-editor\stamp_editor"
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
cargo run --release
# optional: Pack direkt öffnen
cargo run --release -- --open "..\..\Documents\Anno 117 - Pax Romana\stamps\Region Roman\latium.json"
# legacy Zisternen-Pos in allen Packs unter Ordner korrigieren + speichern
cargo run --release -- --fix-cisterns "..\..\Documents\Anno 117 - Pax Romana\stamps"
```

Katalog (lokal, **nicht** im Repo): `..\_gamedata\editor_catalog.json` — aus eigener `assets.xml` erzeugen:

```powershell
python ..\_tools\rebuild_editor_catalog.py
```

Spiel-Stamps (Beispiel): `C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\`

> GitHub enthält absichtlich **keine** entpackten UI-Bilder, keine `assets.xml` und keine abgeleiteten Katalog-JSONs.

## Bedienung

| Aktion | Steuerung |
|--------|-----------|
| Öffnen | Datei → Öffnen oder JSON aufs Fenster ziehen |
| Speichern | Datei → Speichern (auch Autosave bei Änderungen) |
| Pan | Mausrad-Taste / RMB leer |
| Zoom | Scroll |
| Auswählen | LMB auf Gebäude |
| Verschieben | LMB ziehen |
| Platzieren | Gebäude in Palette wählen, LMB auf leere Fläche |
| GUID ersetzen | Palette wählen, RMB auf Gebäude |
| Land/Marsh | **V** beim Platzieren oder auf ausgewähltem Gebäude (nur mit Marsh-Größe, z. B. Lager) |

## Tests / Fixtures

```powershell
cargo test --release
```

Roundtrip- und Layout-Fixtures liegen unter `testdata/` (Albion, Latium, unsorted). Katalog bleibt in `..\_gamedata\`.

## Architektur

| Modul | Rolle |
|-------|------|
| `filedb` | FileDB V3 Reader/Writer (byte-identischer Round-Trip) |
| `stamp` / `pack` | PipetteData + Stamp-Pack JSON |
| `catalog` | GUIDs, Größen, Influence, Factory I/O, ModuleLimits |
| `coverage` | Wegegraph + Radius-Overlay |
| `modules` | ComplexOwner / Farm↔Feld-Zählung |
| `economy` | Ketten-Balance |
| `app` | egui UI |

## Layout-Sync / Datenqualität

- Beim Verschieben: `Polygons.RelativePos` und Module mit gleicher `ComplexOwnerID` werden mitbewegt; AABB/TileCount neu berechnet
- Straßen-Coverage liest **MeshGraph-Kanten** (nicht nur BuildingInfo) und klassifiziert Dirt/Stone über `LogicisticSpeedFactor` / TileSet / Street-Template
- Feldzellen aus `SubTileGrid` werden von Mesh-Straßen gefiltert (keine Felder auf Wegen)
- Kettenrechner: `FactoryInputs`/`FactoryOutputs`/`CycleTime` + `NeedsFuelInput` (Fuel GUID 2085)
- Katalog neu bauen: `python ..\_tools\rebuild_editor_catalog.py`

## Offene Restpunkte (priorisiert)

1. **Feld-Malen / SubTileGrid-Edit** — Felder lesen/mitverschieben/mitlöschen ok; freies Zeichnen neuer Feldzellen fehlt.
2. **Aquädukt / Drainage-Semantik** — sofern im Stamp vorhanden: Anzeige ok-ish, gezielte Edit-Tools fehlen.
3. **Library-DnD Feinschliff** — Verschieben zwischen Ordnern funktioniert; Hold-State-UX kann noch robuster.
4. **45°-Diagonale** — Flag vorhanden; Spielkompatibilität je Stamp prüfen bevor als Default.

Behoben kürzlich: Gruppen-Move/`GridOriginWS`; Farm-Löschen ohne Orphans; Speichern per Stamp-Name (kein staler Index); Stamp-Wechsel flusht Autosave; `module_parent_index` nach Abriss/Delete; kein `len()==1`-Polygon-Leak.

Nicht-Bugs: Küchen ohne `ComplexOwnerID` (z. B. Household Olive Oil) zeigen bewusst kein have/need — Fortschritt hängt am Production Field.
