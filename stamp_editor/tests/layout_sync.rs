use stamp_editor::coverage::{build_road_map, RoadKind};
use stamp_editor::meshgraph::{mesh_road_cells, read_mesh_info};
use stamp_editor::pack::open_first_real_stamp;
use stamp_editor::stamp::decode_pipette_b64;
use std::path::PathBuf;

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

#[test]
fn woodcutter_meshgraph_has_dirt_roads() {
    let open =
        open_first_real_stamp(&testdata().join("unsorted/woodcutter.json")).expect("open woodcutter");
    let info = read_mesh_info(&open.stamp.document).expect("mesh info");
    assert!(!info.edges.is_empty(), "expected mesh edges");
    assert_eq!(info.edges[0].guid, 8541, "earthen road guid");

    let cells = mesh_road_cells(&open.stamp.document);
    assert!(
        cells.len() >= 6,
        "expected rasterized road cells, got {}",
        cells.len()
    );

    // Without catalog, coverage helper still extracts cells via document.
    let catalog = stamp_editor::catalog::Catalog::default();
    let roads = build_road_map(&open.stamp.buildings, &catalog, Some(&open.stamp.document));
    assert!(!roads.is_empty());
    assert!(roads.values().all(|k| *k == RoadKind::Dirt));
}

#[test]
fn moving_building_updates_polygon_relative_pos() {
    let pack_path = testdata().join("Albion/albion.json");
    let pack = stamp_editor::pack::load_pack(&pack_path).unwrap();
    let entry = pack
        .stamps
        .iter()
        .find(|s| s.name == "Tunics x1")
        .expect("Tunics x1");
    let mut stamp = decode_pipette_b64(&entry.pipette_data).unwrap();
    assert!(!stamp.buildings.is_empty());
    // Building 0 owns the hemp module polygon via ComplexOwnerID.
    let owner = stamp.buildings[0].complex_owner_id.expect("owner id");
    assert_ne!(owner, 0);

    // Read original polygon RelativePos
    use stamp_editor::filedb::Node;
    fn first_rel(nodes: &[Node]) -> Option<(f32, f32)> {
        for n in nodes {
            if let Node::Tag(t) = n {
                if t.name == "Polygons" {
                    for c in &t.children {
                        if let Node::Tag(e) = c {
                            for a in &e.children {
                                if let Node::Attrib(a) = a {
                                    if a.name == "RelativePos" && a.content.len() >= 8 {
                                        return Some((
                                            f32::from_le_bytes(a.content[0..4].try_into().ok()?),
                                            f32::from_le_bytes(a.content[4..8].try_into().ok()?),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(v) = first_rel(&t.children) {
                    return Some(v);
                }
            }
        }
        None
    }
    let before = first_rel(&stamp.document.roots).expect("polygon RelativePos");
    let old = (stamp.buildings[0].x, stamp.buildings[0].y);
    // Group move (Main+Module/Polygon) is opt-in via move_linked.
    stamp
        .set_building_pos_ex(0, old.0 + 1.0, old.1 + 2.0, true)
        .expect("move");
    assert_eq!(stamp.buildings[0].x, old.0 + 1.0);
    assert_eq!(stamp.buildings[0].y, old.1 + 2.0);
    let after = first_rel(&stamp.document.roots).expect("polygon RelativePos after");
    assert!(
        (after.0 - (before.0 + 1.0)).abs() < 0.01 && (after.1 - (before.1 + 2.0)).abs() < 0.01,
        "polygon should move with ComplexOwnerID group: before={before:?} after={after:?}"
    );

    // Default individual move leaves polygons alone.
    let mid = first_rel(&stamp.document.roots).expect("polygon mid");
    stamp
        .set_building_pos(0, old.0 + 3.0, old.1 + 4.0)
        .expect("individual move");
    let after_ind = first_rel(&stamp.document.roots).expect("polygon after individual");
    assert!(
        (after_ind.0 - mid.0).abs() < 0.01 && (after_ind.1 - mid.1).abs() < 0.01,
        "individual move must not shift polygon: mid={mid:?} after={after_ind:?}"
    );
}
