# Complex-Module / Felder-Zugehörigkeit

Approved by user (chat): restore prior behaviour for farm ↔ field modules.

## Goals

1. **Visual group** — On hover/select of a building with `ComplexOwnerID`, outline the whole complex (main + modules). Main stronger/distinct color; modules secondary.
2. **Count overlay** — On farm mains with `needed_area` / module need: show `have/need` (e.g. `3/3`) near the building on the canvas.
3. **Place-module control** — When a farm (or its module) is selected, show a clear control to place a suggested field module linked to that main (`module_parent_index` + shared owner). Not hidden behind Analyse-only UI.
4. **Field loading** — Ensure polygon/SubTileGrid fields that carry `ComplexOwnerID` participate in grouping/highlight; BuildingInfo modules already do.

## Non-goals

- Redesign of Baulette.
- Changing stamp file format beyond writing `ComplexOwnerID` (already supported).

## Approach

- Reuse `modules.rs` (`complex_group_indices`, `count_modules_for`, `needed_modules`, `suggested_modules`, `module_parent_index` placement path).
- Canvas paint pass: after footprints, if hover/select resolves to a complex, stroke all group footprints; draw `have/need` above main.
- Selection UI: compact strip/window (or always-visible group under canvas when selection is a farm) with `+ Modul` buttons — same logic as Analyse panel.
- `PolygonCell`: optional `complex_owner_id`; extract from polygon entry; tint/highlight cells when their owner matches selected complex.

## Success

- Opening a sheep-farm stamp shows linked fields highlighted with the farm and `n/need` on the farm.
- Selecting the farm offers placing another field that joins the same owner.
