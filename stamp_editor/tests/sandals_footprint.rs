//! Sandals x12: footprint must use absolute-aware tile snap (DragRectCenter.y = *.5).
//! Naïve floor(relative) shifts buildings onto the mesh road frame.

use stamp_editor::catalog::Catalog;
use stamp_editor::coverage::footprint_cells_for_region;
use stamp_editor::meshgraph::{mesh_road_cells, read_mesh_info, relative_to_tile};
use stamp_editor::pack::open_named_stamp;
use std::collections::HashSet;
use std::path::PathBuf;

fn catalog() -> Catalog {
    Catalog::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("_gamedata/editor_catalog.json"),
    )
    .expect("catalog")
}

fn open_sandals() -> stamp_editor::pack::OpenStamp {
    open_named_stamp(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/Latium/latium.json"),
        "Sandals x12",
    )
    .expect("Sandals x12")
}

#[test]
fn relative_to_tile_fixes_half_drag_shift() {
    // Root cause repro: absolute tile 2002 with drag.y=2013.5 → relative geo -11.5.
    // floor(-11.5) = -12 (wrong, onto road). abs-aware → -11.
    assert_eq!(relative_to_tile(-11.5, 2013.5), -11);
    assert_eq!((-11.5_f32).floor() as i32, -12);
    // Integer drag: same as floor.
    assert_eq!(relative_to_tile(-11.5, 2013.0), -12);
    assert_eq!(relative_to_tile(-11.0, 2013.0), -11);
}

#[test]
fn sandals_leather_band_has_no_horizontal_gap() {
    let cat = catalog();
    let open = open_sandals();
    let drag = read_mesh_info(&open.stamp.document)
        .map(|i| i.drag_center)
        .unwrap_or((0.0, 0.0));
    assert!(
        drag.1.fract().abs() > 0.01,
        "Sandals fixture must keep half-integer DragRectCenter.y (got {})",
        drag.1
    );

    let mut leather: HashSet<(i32, i32)> = HashSet::new();
    for b in open.stamp.buildings.iter().filter(|b| b.guid == 2994) {
        for c in footprint_cells_for_region(b, cat.building(b.guid), 0, drag) {
            leather.insert((c.x, c.y));
        }
    }
    assert!(leather.len() > 20, "expected leather footprints");

    let min_y = leather.iter().map(|c| c.1).min().unwrap();
    let max_y = leather.iter().map(|c| c.1).max().unwrap();
    for y in min_y..=max_y {
        let row = leather.iter().filter(|c| c.1 == y).count();
        assert!(
            row > 0,
            "leather band has empty row at y={y} (footprint origin bug; min={min_y} max={max_y})"
        );
    }
}

#[test]
fn sandals_buildings_do_not_overlap_mesh_roads() {
    let cat = catalog();
    let open = open_sandals();
    let drag = read_mesh_info(&open.stamp.document)
        .map(|i| i.drag_center)
        .unwrap_or((0.0, 0.0));
    let roads: HashSet<(i32, i32)> = mesh_road_cells(&open.stamp.document)
        .into_iter()
        .map(|(x, y, _)| (x, y))
        .collect();
    assert!(!roads.is_empty(), "expected mesh roads");

    let mut overlap = 0usize;
    for b in &open.stamp.buildings {
        for c in footprint_cells_for_region(b, cat.building(b.guid), 0, drag) {
            if roads.contains(&(c.x, c.y)) {
                overlap += 1;
            }
        }
    }
    assert_eq!(
        overlap, 0,
        "building footprints must sit inside the road frame, not on it (got {overlap} cells)"
    );
}
