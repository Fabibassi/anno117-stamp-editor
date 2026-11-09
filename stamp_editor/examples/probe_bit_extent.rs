use stamp_editor::filedb::{Attrib, Node, Tag};
use stamp_editor::meshgraph::read_mesh_info;
use stamp_editor::pack::load_pack;
use stamp_editor::stamp::decode_pipette_b64;
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
        .unwrap_or_else(|| "Honeycombs x4 (Flax)".into());
    let pack = load_pack(&PathBuf::from(
        r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\Region Roman\latium.json",
    ))
    .unwrap();
    let entry = pack.stamps.iter().find(|s| s.name == name).unwrap();
    let stamp = decode_pipette_b64(&entry.pipette_data).unwrap();
    let (cx, cy) = read_mesh_info(&stamp.document).unwrap().drag_center;

    for (pi, root) in stamp.document.roots.iter().enumerate() {
        let _ = pi;
        let Node::Tag(t) = root else { continue };
        if t.name != "PipetteData" {
            continue;
        }
        let Some(polys) = find_tag(&t.children, "Polygons") else {
            continue;
        };
        for (i, child) in polys.children.iter().enumerate() {
            let Node::Tag(entry) = child else { continue };
            let guid = find_attrib(&entry.children, "GUID")
                .map(|a| u32::from_le_bytes(a.content[0..4].try_into().unwrap()))
                .unwrap_or(0);
            let rel = find_attrib(&entry.children, "RelativePos").and_then(|a| {
                Some((
                    f32::from_le_bytes(a.content[0..4].try_into().unwrap()),
                    f32::from_le_bytes(a.content[4..8].try_into().unwrap()),
                ))
            });
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
                .map(|a| i32::from_le_bytes(a.content[0..4].try_into().unwrap()) as usize)
                .unwrap();
            let gy = find_attrib(&grid.children, "y")
                .map(|a| i32::from_le_bytes(a.content[0..4].try_into().unwrap()) as usize)
                .unwrap();
            let bits = &find_attrib(&grid.children, "bits").unwrap().content;
            let stride = ((gx + 31) / 32) * 32;
            let ox = ox_abs as f32 - cx;
            let oy = oy_abs as f32 - cy;
            let mut min_sx = gx;
            let mut max_sx = 0usize;
            let mut min_sy = gy;
            let mut max_sy = 0usize;
            let mut set = 0usize;
            let mut min_tx = i32::MAX;
            let mut max_tx = i32::MIN;
            let mut min_ty = i32::MAX;
            let mut max_ty = i32::MIN;
            for sy in 0..gy {
                for sx in 0..gx {
                    if !bit(bits, sy * stride + sx) {
                        continue;
                    }
                    set += 1;
                    min_sx = min_sx.min(sx);
                    max_sx = max_sx.max(sx);
                    min_sy = min_sy.min(sy);
                    max_sy = max_sy.max(sy);
                    let tx = (ox + sx as f32 * 0.5).floor() as i32;
                    let ty = (oy + sy as f32 * 0.5).floor() as i32;
                    min_tx = min_tx.min(tx);
                    max_tx = max_tx.max(tx);
                    min_ty = min_ty.min(ty);
                    max_ty = max_ty.max(ty);
                }
            }
            // Also try stride = gx (no pad)
            let mut set2 = 0usize;
            let mut max_tx2 = i32::MIN;
            for sy in 0..gy {
                for sx in 0..gx {
                    if !bit(bits, sy * gx + sx) {
                        continue;
                    }
                    set2 += 1;
                    max_tx2 = max_tx2.max((ox + sx as f32 * 0.5).floor() as i32);
                }
            }
            println!(
                "poly{i} guid={guid} rel={rel:?} origin_rel=({ox},{oy}) grid={gx}x{gy} bits_len={} stride={stride}",
                bits.len()
            );
            println!(
                "  set={set} sx={min_sx}..{max_sx} sy={min_sy}..{max_sy} tiles=({min_tx},{min_ty})..({max_tx},{max_ty})"
            );
            println!("  nopad set={set2} max_tx={max_tx2}");
            // print compact bitmask of set density per column (8-wide bins)
            let bins = (gx + 7) / 8;
            let mut col = vec![0usize; bins];
            for sy in 0..gy {
                for sx in 0..gx {
                    if bit(bits, sy * stride + sx) {
                        col[sx / 8] += 1;
                    }
                }
            }
            print!("  col_density(pad):");
            for c in &col {
                print!(" {c}");
            }
            println!();
        }
    }
}
