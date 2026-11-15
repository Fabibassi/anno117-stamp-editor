use stamp_editor::meshgraph::{
    apply_road_path, is_aqueduct_maxground_guid, mesh_road_cells, read_mesh_info,
};
use stamp_editor::stamp::Stamp;

#[test]
fn aqueduct_coalesce_and_maxground_over_road() {
    let mut stamp = Stamp::blank().expect("blank");
    // Horizontal road
    let road: Vec<(i32, i32)> = (-2..=2).map(|x| (x, 0)).collect();
    apply_road_path(
        &mut stamp.document,
        &stamp.buildings,
        &road,
        true,
        23996,
    );
    // Vertical aqueduct crossing the road at (0,0)
    let aq: Vec<(i32, i32)> = (-3..=3).map(|y| (0, y)).collect();
    apply_road_path(
        &mut stamp.document,
        &stamp.buildings,
        &aq,
        true,
        19723,
    );

    let info = read_mesh_info(&stamp.document).expect("mesh");
    assert!(
        info.edges.len() < 12,
        "expected coalesced edges, got {}",
        info.edges.len()
    );
    assert!(
        info.edges.iter().any(|e| is_aqueduct_maxground_guid(e.guid)),
        "expected MaxGround over road, edges={:?}",
        info.edges
            .iter()
            .map(|e| e.guid)
            .collect::<Vec<_>>()
    );
    assert!(
        info.edges
            .iter()
            .any(|e| e.guid == 19723 && (e.y1 - e.y0).abs() >= 2),
        "expected a long MinGround span"
    );

    // Round-trip cell count: aqueduct column must still be present
    let cells = mesh_road_cells(&stamp.document);
    let aq_cells: Vec<_> = cells
        .iter()
        .filter(|(_, _, g)| *g == 19723 || is_aqueduct_maxground_guid(*g))
        .map(|(x, y, _)| (*x, *y))
        .collect();
    for y in -3..=3 {
        assert!(
            aq_cells.contains(&(0, y)),
            "missing aqueduct cell (0,{y}) in {:?}",
            aq_cells
        );
    }
}
