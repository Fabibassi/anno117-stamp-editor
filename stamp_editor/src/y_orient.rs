//! Anno stamp space is Y-up. Early editor builds painted Y-down.
//! Reflecting with `y → -y` on centres breaks even-sized footprints; use tile
//! reflection `y → -y - 1` (maps unit square [k,k+1) to [-k-1,-k)).

use crate::meshgraph::{
    apply_road_path, clear_all_mesh_paths, mesh_road_cells, relative_to_tile,
};
use crate::stamp::Stamp;
use std::collections::BTreeMap;

pub fn reflect_tile_y(y: i32) -> i32 {
    -y - 1
}

fn footprint_wh(guid: u32) -> (i32, i32) {
    if matches!(guid, 19753 | 29526 | 149794) {
        (3, 3)
    } else {
        // Safe default for this project's common 2×2 service/residence tiles.
        (2, 2)
    }
}

/// Reflect stamp content through Y (tile-accurate).
///
/// Mesh must be **cleared then rewritten** from the reflected cell set. Cell-wise
/// demolish leaves rasterization leftovers; coalesce then welds old+new into long
/// ghost roads (in-game: paths floating beside buildings).
pub fn reflect_stamp_y(stamp: &mut Stamp) {
    let drag = crate::meshgraph::read_mesh_info(&stamp.document)
        .map(|i| i.drag_center)
        .unwrap_or((0.0, 0.0));
    let cells = mesh_road_cells(&stamp.document);
    let mut by_guid: BTreeMap<i32, Vec<(i32, i32)>> = BTreeMap::new();
    for (x, y, g) in &cells {
        by_guid
            .entry(*g)
            .or_default()
            .push((*x, reflect_tile_y(*y)));
    }
    for v in by_guid.values_mut() {
        v.sort();
        v.dedup();
    }

    let n = stamp.buildings.len();
    let plans: Vec<_> = stamp
        .buildings
        .iter()
        .map(|b| {
            let (_w, h) = footprint_wh(b.guid);
            let y0 = relative_to_tile(b.y - h as f32 * 0.5, drag.1);
            let new_y0 = -y0 - h;
            let new_y = new_y0 as f32 + h as f32 * 0.5;
            let new_dir = match b.direction.rem_euclid(8) {
                0 => 4,
                4 => 0,
                d => d,
            };
            (b.x, new_y, new_dir)
        })
        .collect();
    for i in 0..n {
        let (x, y, dir) = plans[i];
        let _ = stamp.set_building_pos(i, x, y);
        let _ = stamp.set_building_direction(i, dir);
    }

    // Full wipe — never demolish-by-cell before re-paint (ghost spans).
    let _ = clear_all_mesh_paths(&mut stamp.document, &stamp.buildings);
    for (g, pts) in &by_guid {
        apply_road_path(&mut stamp.document, &stamp.buildings, pts, true, *g);
    }
    stamp.refresh_buildings();
}
