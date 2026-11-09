//! Olive grove fields: crop icons + never paint on roads.

use stamp_editor::meshgraph::mesh_road_cells;
use stamp_editor::pack::open_named_stamp;
use std::collections::HashSet;
use std::path::PathBuf;

fn latium() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/Latium/latium.json")
}

#[test]
fn olive_oil_fields_do_not_sit_on_roads() {
    let open = open_named_stamp(&latium(), "Olive Oil x3 v2").expect("Olive Oil x3 v2");
    assert!(
        open.stamp.polygon_cells.len() > 100,
        "expected olive grove SubTileGrid cells, got {}",
        open.stamp.polygon_cells.len()
    );
    let roads: HashSet<(i32, i32)> = mesh_road_cells(&open.stamp.document)
        .into_iter()
        .map(|(x, y, _)| (x, y))
        .collect();
    let overlap = open
        .stamp
        .polygon_cells
        .iter()
        .filter(|c| roads.contains(&(c.x, c.y)))
        .count();
    assert_eq!(
        overlap, 0,
        "decoded field tiles must not sit on mesh roads (got {overlap})"
    );
    assert!(
        open.stamp.polygon_cells.iter().all(|c| c.guid == 2745),
        "olive field cells should be Module Field Roman Olives"
    );
}
