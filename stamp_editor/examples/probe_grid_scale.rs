//! Probe which SubTileGrid decode keeps cells inside the road AABB.
use stamp_editor::coverage::footprint_cells;
use stamp_editor::meshgraph::{mesh_road_cells, read_mesh_info};
use stamp_editor::pack::load_pack;
use stamp_editor::stamp::decode_pipette_b64;
use stamp_editor::filedb::{Attrib, Node, Tag};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn find_attrib<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Attrib> {
    nodes.iter().find_map(|n| match n {
        Node::Attrib(a) if a.name == name => Some(a),
        _ => None,
    })
}
fn find_tag<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Tag> {
    nodes.iter().find_map(|n| match n {
        Node::Tag(t) if t.name == name => Some(t),
        _ => None,
    })
}
fn bit(bits: &[u8], idx: usize) -> bool {
    bits.get(idx / 8).map(|b| (b >> (idx % 8)) & 1 == 1).unwrap_or(false)
}

fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Loungers x4".into());
    let pack = load_pack(&PathBuf::from(
        r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\Region Roman\latium.json",
    ))
    .unwrap();
    let entry = pack.stamps.iter().find(|s| s.name == name).unwrap();
    let stamp = decode_pipette_b64(&entry.pipette_data).unwrap();
    let info = read_mesh_info(&stamp.document).unwrap();
    let (cx, cy) = info.drag_center;
    let roads: BTreeSet<_> = mesh_road_cells(&stamp.document)
        .into_iter()
        .map(|(x, y, _)| (x, y))
        .collect();
    let (rmin_x, rmax_x, rmin_y, rmax_y) = {
        let xs: Vec<_> = roads.iter().map(|c| c.0).collect();
        let ys: Vec<_> = roads.iter().map(|c| c.1).collect();
        (
            *xs.iter().min().unwrap(),
            *xs.iter().max().unwrap(),
            *ys.iter().min().unwrap(),
            *ys.iter().max().unwrap(),
        )
    };
    println!(
        "{name} road AABB x={rmin_x}..{rmax_x} y={rmin_y}..{rmax_y} drag={cx},{cy}"
    );

    // building AABB
    let mut bmin_x = i32::MAX;
    let mut bmax_x = i32::MIN;
    let mut bmin_y = i32::MAX;
    let mut bmax_y = i32::MIN;
    for b in &stamp.buildings {
        for c in footprint_cells(b, None) {
            bmin_x = bmin_x.min(c.x);
            bmax_x = bmax_x.max(c.x);
            bmin_y = bmin_y.min(c.y);
            bmax_y = bmax_y.max(c.y);
        }
    }
    println!("building AABB x={bmin_x}..{bmax_x} y={bmin_y}..{bmax_y}");

    for scale in [0.5_f32, 1.0] {
        for pad32 in [false, true] {
            for require4 in [false, true] {
                let mut cells = BTreeSet::new();
                for root in &stamp.document.roots {
                    let Node::Tag(t) = root else { continue };
                    if t.name != "PipetteData" {
                        continue;
                    }
                    let Some(polys) = find_tag(&t.children, "Polygons") else {
                        continue;
                    };
                    for child in &polys.children {
                        let Node::Tag(entry) = child else { continue };
                        let Some(stg) = find_tag(&entry.children, "SubTileGrid") else {
                            continue;
                        };
                        let origin = find_attrib(&stg.children, "GridOriginWS").unwrap();
                        let ox_abs = i32::from_le_bytes(origin.content[0..4].try_into().unwrap());
                        let oy_abs = i32::from_le_bytes(origin.content[4..8].try_into().unwrap());
                        let grid = find_tag(&stg.children, "Grid")
                            .and_then(|g| find_tag(&g.children, "grid"))
                            .unwrap();
                        let gx = find_attrib(&grid.children, "x")
                            .map(|a| i32::from_le_bytes(a.content[0..4].try_into().unwrap()))
                            .unwrap() as usize;
                        let gy = find_attrib(&grid.children, "y")
                            .map(|a| i32::from_le_bytes(a.content[0..4].try_into().unwrap()))
                            .unwrap() as usize;
                        let bits = &find_attrib(&grid.children, "bits").unwrap().content;
                        let ox = ox_abs as f32 - cx;
                        let oy = oy_abs as f32 - cy;
                        let stride = if pad32 {
                            ((gx + 31) / 32) * 32
                        } else {
                            gx
                        };
                        if require4 {
                            let tw = ((gx as f32) * scale).ceil() as i32;
                            let th = ((gy as f32) * scale).ceil() as i32;
                            for ty in 0..th {
                                for tx in 0..tw {
                                    let mut ok = true;
                                    for dy in 0..2 {
                                        for dx in 0..2 {
                                            let sx = ((tx as f32) / scale) as usize + dx;
                                            let sy = ((ty as f32) / scale) as usize + dy;
                                            if sx >= gx || sy >= gy {
                                                ok = false;
                                                break;
                                            }
                                            let idx = sy * stride + sx;
                                            if !bit(bits, idx) {
                                                ok = false;
                                                break;
                                            }
                                        }
                                    }
                                    if ok {
                                        cells.insert((
                                            (ox + tx as f32).floor() as i32,
                                            (oy + ty as f32).floor() as i32,
                                        ));
                                    }
                                }
                            }
                        } else {
                            for sy in 0..gy {
                                for sx in 0..gx {
                                    let idx = sy * stride + sx;
                                    if !bit(bits, idx) {
                                        continue;
                                    }
                                    cells.insert((
                                        (ox + sx as f32 * scale).floor() as i32,
                                        (oy + sy as f32 * scale).floor() as i32,
                                    ));
                                }
                            }
                        }
                    }
                }
                let outside = cells
                    .iter()
                    .filter(|c| {
                        c.0 < rmin_x - 1
                            || c.0 > rmax_x + 1
                            || c.1 < rmin_y - 1
                            || c.1 > rmax_y + 1
                    })
                    .count();
                let inside_empty_wings = cells
                    .iter()
                    .filter(|c| {
                        // left of buildings or right of buildings, inside road
                        (c.0 < bmin_x || c.0 > bmax_x)
                            && c.0 >= rmin_x
                            && c.0 <= rmax_x
                            && c.1 >= rmin_y
                            && c.1 <= rmax_y
                    })
                    .count();
                println!(
                    "scale={scale} pad32={pad32} req4={require4} cells={} outside_road={} wing_cells={}",
                    cells.len(),
                    outside,
                    inside_empty_wings
                );
            }
        }
    }
}
