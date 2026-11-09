//! SubTileGrid: each bit-row is one tile in Y; every 4 bit-columns are one tile in X.
//! Honeycombs x4 (Lavender) must decode so fields+buildings fill the road rectangle
//! and each lavender farm reports 180/180 — without inventing tiles.

use stamp_editor::catalog::Catalog;
use stamp_editor::coverage::footprint_cells_for_region;
use stamp_editor::meshgraph::{mesh_road_cells, read_mesh_info};
use stamp_editor::modules::{count_progress_for_stamp_area, needed_modules, uses_area_progress};
use stamp_editor::pack::open_named_stamp;
use std::collections::HashSet;
use std::path::PathBuf;

fn latium() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/Latium/latium.json")
}

fn catalog() -> Catalog {
    Catalog::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("_gamedata/editor_catalog.json"),
    )
    .expect("catalog")
}

#[test]
fn honeycombs_lavender_fills_road_rectangle_at_180() {
    let cat = catalog();
    let open = open_named_stamp(&latium(), "Honeycombs x4 (Lavender)").expect("stamp");
    let drag = read_mesh_info(&open.stamp.document)
        .map(|i| i.drag_center)
        .unwrap_or((0.0, 0.0));
    let roads: HashSet<(i32, i32)> = mesh_road_cells(&open.stamp.document)
        .into_iter()
        .map(|(x, y, _)| (x, y))
        .collect();
    assert!(!roads.is_empty());
    let min_x = roads.iter().map(|c| c.0).min().unwrap();
    let max_x = roads.iter().map(|c| c.0).max().unwrap();
    let min_y = roads.iter().map(|c| c.1).min().unwrap();
    let max_y = roads.iter().map(|c| c.1).max().unwrap();

    let fields: HashSet<(i32, i32)> = open
        .stamp
        .polygon_cells
        .iter()
        .map(|c| (c.x, c.y))
        .collect();
    assert_eq!(
        fields.iter().filter(|c| roads.contains(c)).count(),
        0,
        "decoded fields must not sit on roads"
    );
    let outside = fields
        .iter()
        .filter(|c| c.0 < min_x || c.0 > max_x || c.1 < min_y || c.1 > max_y)
        .count();
    assert_eq!(outside, 0, "decoded fields must stay inside the road frame");

    let mut bld = HashSet::new();
    for b in open
        .stamp
        .buildings
        .iter()
        .chain(open.stamp.polygon_modules.iter())
    {
        for c in footprint_cells_for_region(b, cat.building(b.guid), 0, drag) {
            bld.insert((c.x, c.y));
        }
    }

    let mut interior = 0usize;
    let mut empty = 0usize;
    for y in (min_y + 1)..max_y {
        for x in (min_x + 1)..max_x {
            if roads.contains(&(x, y)) {
                continue;
            }
            interior += 1;
            if !fields.contains(&(x, y)) && !bld.contains(&(x, y)) {
                empty += 1;
            }
        }
    }
    let covered = interior - empty;
    let fill_pct = 100.0 * covered as f32 / interior as f32;
    assert!(
        fill_pct >= 90.0,
        "fields+buildings must fill the road rectangle (got {fill_pct:.1}% covered, {empty} empty of {interior})"
    );

    let mut lavender = 0usize;
    for (i, b) in open.stamp.buildings.iter().enumerate() {
        if b.guid != 2698 {
            continue;
        }
        let def = cat.building(b.guid);
        assert!(uses_area_progress(def));
        assert_eq!(needed_modules(def), 180);
        let have = count_progress_for_stamp_area(
            &open.stamp.buildings,
            &open.stamp.polygon_modules,
            &open.stamp.polygon_cells,
            &open.stamp.polygon_area,
            i,
            def,
        );
        assert_eq!(have, 180, "lavender farm[{i}] must be 180/180 after correct decode");
        lavender += 1;
    }
    assert_eq!(lavender, 4);
}

#[test]
fn soap_lavender_area_progress_is_full() {
    let cat = catalog();
    let open = open_named_stamp(&latium(), "Soap x4").expect("Soap x4");
    let mut checked = 0usize;
    for (i, b) in open.stamp.buildings.iter().enumerate() {
        if b.guid != 2698 {
            continue;
        }
        let def = cat.building(b.guid);
        let have = count_progress_for_stamp_area(
            &open.stamp.buildings,
            &open.stamp.polygon_modules,
            &open.stamp.polygon_cells,
            &open.stamp.polygon_area,
            i,
            def,
        );
        assert!(
            have >= 160,
            "soap lavender farm[{i}] area {have}/180 — set_bits/4 decode broken?"
        );
        checked += 1;
    }
    assert!(checked >= 4);
}
