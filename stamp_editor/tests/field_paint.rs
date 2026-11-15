//! PlantFarm SubTileGrid field brush — set/clear tiles with encode roundtrip.

use stamp_editor::catalog::Catalog;
use stamp_editor::coverage::footprint_cells_for_region;
use stamp_editor::meshgraph::{mesh_road_cells, read_mesh_info};
use stamp_editor::modules::complex_main_index;
use stamp_editor::pack::open_named_stamp;
use stamp_editor::stamp::{
    apply_field_tiles, connected_field_subset, decode_pipette_b64, encode_pipette_b64,
    field_cells_of_owner, foreign_field_cells,
};
use std::collections::HashSet;
use std::path::PathBuf;

fn latium() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/Latium/latium.json")
}

fn porridge() -> stamp_editor::pack::OpenStamp {
    open_named_stamp(&latium(), "Porridge x4").expect("Porridge x4")
}

fn catalog() -> Catalog {
    Catalog::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("_gamedata/editor_catalog.json"),
    )
    .expect("catalog")
}

fn road_set(doc: &stamp_editor::filedb::DocumentOrdered) -> HashSet<(i32, i32)> {
    mesh_road_cells(doc)
        .into_iter()
        .map(|(x, y, _)| (x, y))
        .collect()
}

fn main_seeds(
    open: &stamp_editor::pack::OpenStamp,
    cat: &Catalog,
    owner: u64,
) -> HashSet<(i32, i32)> {
    let mut seeds = HashSet::new();
    let drag = read_mesh_info(&open.stamp.document)
        .map(|i| i.drag_center)
        .unwrap_or((0.0, 0.0));
    for (i, b) in open.stamp.buildings.iter().enumerate() {
        if b.complex_owner_id != Some(owner) {
            continue;
        }
        if complex_main_index(&open.stamp.buildings, cat, i) != Some(i) {
            continue;
        }
        for c in footprint_cells_for_region(b, cat.building(b.guid), open.region_guid, drag) {
            seeds.insert((c.x, c.y));
        }
    }
    seeds
}

#[test]
fn paint_adds_tile_and_survives_roundtrip() {
    let mut open = porridge();
    let cat = catalog();
    let farm_i = open
        .stamp
        .buildings
        .iter()
        .position(|b| b.guid == 2200)
        .expect("oats farm");
    let owner = open.stamp.buildings[farm_i]
        .complex_owner_id
        .expect("owner");
    let before: HashSet<(i32, i32)> = field_cells_of_owner(&open.stamp, owner);
    assert!(!before.is_empty());

    let roads = road_set(&open.stamp.document);
    let seed = *before.iter().min().unwrap();
    let candidate = [(1, 0), (-1, 0), (0, 1), (0, -1), (2, 0), (0, 2), (3, 0), (0, 3), (4, 0), (0, 4)]
        .into_iter()
        .map(|(dx, dy)| (seed.0 + dx, seed.1 + dy))
        .find(|p| !before.contains(p) && !roads.contains(p))
        .expect("free neighbour");

    let seeds = main_seeds(&open, &cat, owner);
    let n = apply_field_tiles(
        &mut open.stamp,
        owner,
        2220,
        &[candidate],
        true,
        &roads,
        Some(&seeds),
    );
    assert_eq!(n, 1, "should paint one tile");
    open.stamp.refresh_buildings();

    let after = field_cells_of_owner(&open.stamp, owner);
    assert!(after.contains(&candidate), "painted tile missing in decode");
    assert_eq!(after.len(), before.len() + 1);

    let area = *open.stamp.polygon_area.get(&owner).unwrap_or(&0);
    assert_eq!(area as usize, after.len(), "polygon_area should match unique tiles");

    let enc = encode_pipette_b64(&open.stamp).expect("encode");
    let again = decode_pipette_b64(&enc).expect("decode");
    let persisted = field_cells_of_owner(&again, owner);
    assert!(
        persisted.contains(&candidate),
        "saved pipette must keep painted field"
    );
    assert_eq!(persisted.len(), after.len());
}

#[test]
fn erase_removes_tile() {
    let mut open = porridge();
    let cat = catalog();
    let farm_i = open
        .stamp
        .buildings
        .iter()
        .position(|b| b.guid == 2200)
        .expect("oats farm");
    let owner = open.stamp.buildings[farm_i]
        .complex_owner_id
        .expect("owner");
    let cell = *field_cells_of_owner(&open.stamp, owner)
        .iter()
        .next()
        .expect("field cell");
    let before = field_cells_of_owner(&open.stamp, owner).len();
    let seeds = main_seeds(&open, &cat, owner);

    let n = apply_field_tiles(
        &mut open.stamp,
        owner,
        2220,
        &[cell],
        false,
        &HashSet::new(),
        Some(&seeds),
    );
    assert!(n >= 1);
    open.stamp.refresh_buildings();
    let after = field_cells_of_owner(&open.stamp, owner);
    assert!(!after.contains(&cell));
    assert!(after.len() < before);
}

#[test]
fn paint_grows_grid_outside_extent_when_adjacent() {
    let mut open = porridge();
    let cat = catalog();
    let farm_i = open
        .stamp
        .buildings
        .iter()
        .position(|b| b.guid == 2200)
        .expect("oats farm");
    let owner = open.stamp.buildings[farm_i]
        .complex_owner_id
        .expect("owner");
    let before = field_cells_of_owner(&open.stamp, owner);
    let (min_x, min_y) = before.iter().fold((i32::MAX, i32::MAX), |(ax, ay), (x, y)| {
        (ax.min(*x), ay.min(*y))
    });
    // Adjacent west of the current blob → grows extent, stays connected.
    let edge = (min_x - 1, min_y);
    let roads = road_set(&open.stamp.document);
    let seeds = main_seeds(&open, &cat, owner);

    let n = apply_field_tiles(
        &mut open.stamp,
        owner,
        2220,
        &[edge],
        true,
        &roads,
        Some(&seeds),
    );
    assert_eq!(n, 1);
    open.stamp.refresh_buildings();
    let after = field_cells_of_owner(&open.stamp, owner);
    assert!(after.contains(&edge), "grown tile missing");
    for c in &before {
        assert!(after.contains(c), "lost cell {c:?} after grow");
    }
}

#[test]
fn paint_skips_road_blocker() {
    let mut open = porridge();
    let cat = catalog();
    let farm_i = open
        .stamp
        .buildings
        .iter()
        .position(|b| b.guid == 2200)
        .expect("oats farm");
    let owner = open.stamp.buildings[farm_i]
        .complex_owner_id
        .expect("owner");
    let roads = road_set(&open.stamp.document);
    assert!(!roads.is_empty(), "fixture needs roads");
    let road = *roads.iter().next().unwrap();
    let before = field_cells_of_owner(&open.stamp, owner).len();
    let seeds = main_seeds(&open, &cat, owner);

    let n = apply_field_tiles(
        &mut open.stamp,
        owner,
        2220,
        &[road],
        true,
        &roads,
        Some(&seeds),
    );
    assert_eq!(n, 0, "road tiles must not be painted");
    open.stamp.refresh_buildings();
    assert_eq!(field_cells_of_owner(&open.stamp, owner).len(), before);
}

#[test]
fn paint_rejects_disconnected_island() {
    let mut open = porridge();
    let cat = catalog();
    let farm_i = open
        .stamp
        .buildings
        .iter()
        .position(|b| b.guid == 2200)
        .expect("oats farm");
    let owner = open.stamp.buildings[farm_i]
        .complex_owner_id
        .expect("owner");
    let before = field_cells_of_owner(&open.stamp, owner);
    let (min_x, min_y) = before.iter().fold((i32::MAX, i32::MAX), |(ax, ay), (x, y)| {
        (ax.min(*x), ay.min(*y))
    });
    let island = (min_x - 8, min_y - 8);
    let roads = road_set(&open.stamp.document);
    let seeds = main_seeds(&open, &cat, owner);
    let n = apply_field_tiles(
        &mut open.stamp,
        owner,
        2220,
        &[island],
        true,
        &roads,
        Some(&seeds),
    );
    assert_eq!(n, 0, "disconnected island must be rejected");
    open.stamp.refresh_buildings();
    assert_eq!(field_cells_of_owner(&open.stamp, owner), before);
}

#[test]
fn paint_rejects_foreign_farm_overlap() {
    let mut open = porridge();
    let cat = catalog();
    let oats: Vec<_> = open
        .stamp
        .buildings
        .iter()
        .enumerate()
        .filter(|(_, b)| b.guid == 2200)
        .map(|(i, b)| (i, b.complex_owner_id.expect("owner")))
        .collect();
    assert!(oats.len() >= 2, "need two farms");
    let owner_a = oats[0].1;
    let owner_b = oats[1].1;
    assert_ne!(owner_a, owner_b);

    let cell_b = *field_cells_of_owner(&open.stamp, owner_b)
        .iter()
        .next()
        .expect("B field");
    let roads = road_set(&open.stamp.document);
    let seeds = main_seeds(&open, &cat, owner_a);
    let before_a = field_cells_of_owner(&open.stamp, owner_a).len();

    let n = apply_field_tiles(
        &mut open.stamp,
        owner_a,
        2220,
        &[cell_b],
        true,
        &roads,
        Some(&seeds),
    );
    assert_eq!(n, 0, "must not steal another farm's tile");
    open.stamp.refresh_buildings();
    assert_eq!(field_cells_of_owner(&open.stamp, owner_a).len(), before_a);
    assert!(foreign_field_cells(&open.stamp, owner_a).contains(&cell_b));
}

#[test]
fn connected_subset_grows_from_seeds() {
    let seeds = HashSet::from([(0, 0)]);
    let cand = HashSet::from([(1, 0), (2, 0), (5, 5)]);
    let ok = connected_field_subset(&seeds, &cand);
    assert_eq!(ok, HashSet::from([(1, 0), (2, 0)]));
}
