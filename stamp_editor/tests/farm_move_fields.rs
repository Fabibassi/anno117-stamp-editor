//! Data-loss guard: group-moving a PlantFarm must shift SubTileGrid field cells.

use stamp_editor::meshgraph::mesh_road_cells;
use stamp_editor::pack::open_named_stamp;
use stamp_editor::stamp::{decode_pipette_b64, encode_pipette_b64};
use std::collections::HashSet;
use std::path::PathBuf;

fn olive() -> stamp_editor::pack::OpenStamp {
    open_named_stamp(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/Latium/latium.json"),
        "Olive Oil x3 v2",
    )
    .expect("Olive Oil x3 v2")
}

#[test]
fn linked_farm_move_shifts_subtile_field_cells() {
    let mut open = olive();
    let i = open
        .stamp
        .buildings
        .iter()
        .position(|b| b.guid == 2699)
        .expect("Production Field Roman Olives");
    let owner = open.stamp.buildings[i].complex_owner_id.expect("owner");
    let before: HashSet<(i32, i32)> = open
        .stamp
        .polygon_cells
        .iter()
        .filter(|c| c.complex_owner_id == Some(owner))
        .map(|c| (c.x, c.y))
        .collect();
    assert!(before.len() > 20, "expected field tiles, got {}", before.len());

    let (ox, oy) = (open.stamp.buildings[i].x, open.stamp.buildings[i].y);
    let dx = 2.0_f32;
    let dy = 3.0_f32;
    open.stamp
        .set_building_pos_ex(i, ox + dx, oy + dy, true)
        .expect("linked move");
    open.stamp.refresh_buildings();

    let after: HashSet<(i32, i32)> = open
        .stamp
        .polygon_cells
        .iter()
        .filter(|c| c.complex_owner_id == Some(owner))
        .map(|c| (c.x, c.y))
        .collect();
    // Decode hides field bits that land on roads after the shift.
    let roads: HashSet<(i32, i32)> = mesh_road_cells(&open.stamp.document)
        .into_iter()
        .map(|(x, y, _)| (x, y))
        .collect();
    let expected: HashSet<(i32, i32)> = before
        .iter()
        .map(|(x, y)| (x + dx as i32, y + dy as i32))
        .filter(|p| !roads.contains(p))
        .collect();
    assert_eq!(
        after, expected,
        "SubTileGrid cells must follow ComplexOwner group move (minus road overlap)"
    );
    assert!(
        after.len() + 3 >= before.len(),
        "at most a few cells may fall under roads after shift (got {} → {})",
        before.len(),
        after.len()
    );

    // Persist through encode → decode (what the game would see after save).
    let enc = encode_pipette_b64(&open.stamp).expect("encode");
    let again = decode_pipette_b64(&enc).expect("decode");
    let owner2 = again
        .buildings
        .iter()
        .find(|b| b.guid == 2699 && (b.x - (ox + dx)).abs() < 0.01)
        .and_then(|b| b.complex_owner_id)
        .expect("moved farm");
    let persisted: HashSet<(i32, i32)> = again
        .polygon_cells
        .iter()
        .filter(|c| c.complex_owner_id == Some(owner2))
        .map(|c| (c.x, c.y))
        .collect();
    assert_eq!(persisted, expected, "saved pipette must keep shifted fields");
}

#[test]
fn deleting_farm_main_drops_owned_field_polygons() {
    let mut open = olive();
    let i = open
        .stamp
        .buildings
        .iter()
        .position(|b| b.guid == 2699)
        .expect("Production Field Roman Olives");
    let owner = open.stamp.buildings[i].complex_owner_id.expect("owner");
    let before = open
        .stamp
        .polygon_cells
        .iter()
        .filter(|c| c.complex_owner_id == Some(owner))
        .count();
    assert!(before > 20);

    open.stamp.remove_building(i).expect("remove farm");
    open.stamp.refresh_buildings();

    let orphans = open
        .stamp
        .polygon_cells
        .iter()
        .filter(|c| c.complex_owner_id == Some(owner))
        .count();
    assert_eq!(
        orphans, 0,
        "deleting the only BuildingInfo with owner must drop its SubTileGrid polygons"
    );

    let enc = encode_pipette_b64(&open.stamp).expect("encode");
    let again = decode_pipette_b64(&enc).expect("decode");
    let still = again
        .polygon_cells
        .iter()
        .filter(|c| c.complex_owner_id == Some(owner))
        .count();
    assert_eq!(still, 0, "orphans must not survive save");
}
