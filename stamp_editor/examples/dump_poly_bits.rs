use stamp_editor::filedb::{Attrib, Node, Tag};
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

fn main() {
    let pack = load_pack(&PathBuf::from(
        r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\Region Roman\latium.json",
    ))
    .unwrap();
    let entry = pack
        .stamps
        .iter()
        .find(|s| s.name == "Loungers x4")
        .unwrap();
    let stamp = decode_pipette_b64(&entry.pipette_data).unwrap();
    for root in &stamp.document.roots {
        let Node::Tag(t) = root else { continue };
        if t.name != "PipetteData" {
            continue;
        }
        let Some(polys) = find_tag(&t.children, "Polygons") else {
            continue;
        };
        for (pi, child) in polys.children.iter().enumerate() {
            let Node::Tag(entry) = child else { continue };
            if entry.name != "None" {
                continue;
            }
            let guid = find_attrib(&entry.children, "GUID")
                .map(|a| u32::from_le_bytes(a.content[0..4].try_into().unwrap()))
                .unwrap_or(0);
            let rel = find_attrib(&entry.children, "RelativePos").and_then(|a| {
                if a.content.len() >= 8 {
                    Some((
                        f32::from_le_bytes(a.content[0..4].try_into().unwrap()),
                        f32::from_le_bytes(a.content[4..8].try_into().unwrap()),
                    ))
                } else {
                    None
                }
            });
            let Some(stg) = find_tag(&entry.children, "SubTileGrid") else {
                println!("poly{pi} guid={guid} rel={rel:?} NO SubTileGrid");
                continue;
            };
            let origin = find_attrib(&stg.children, "GridOriginWS").map(|a| a.content.clone());
            let grid = find_tag(&stg.children, "Grid")
                .and_then(|g| find_tag(&g.children, "grid"));
            let Some(grid) = grid else {
                println!("poly{pi} guid={guid} no grid");
                continue;
            };
            let gx = find_attrib(&grid.children, "x")
                .map(|a| i32::from_le_bytes(a.content[0..4].try_into().unwrap()))
                .unwrap_or(0);
            let gy = find_attrib(&grid.children, "y")
                .map(|a| i32::from_le_bytes(a.content[0..4].try_into().unwrap()))
                .unwrap_or(0);
            let bits = find_attrib(&grid.children, "bits").map(|a| a.content.clone());
            println!(
                "poly{pi} guid={guid} rel={rel:?} origin_len={:?} grid={gx}x{gy} bits_len={:?}",
                origin.as_ref().map(|o| o.len()),
                bits.as_ref().map(|b| b.len())
            );
            if let Some(o) = &origin {
                print!("  origin bytes:");
                for b in o.iter().take(32) {
                    print!(" {b:02x}");
                }
                println!();
                if o.len() >= 8 {
                    let x = f32::from_le_bytes(o[0..4].try_into().unwrap());
                    let y = f32::from_le_bytes(o[4..8].try_into().unwrap());
                    println!("  origin as f32: ({x}, {y})");
                    let ix = i32::from_le_bytes(o[0..4].try_into().unwrap());
                    let iy = i32::from_le_bytes(o[4..8].try_into().unwrap());
                    println!("  origin as i32: ({ix}, {iy})");
                }
            }
            if let Some(b) = &bits {
                print!("  bits hex:");
                for x in b.iter().take(64) {
                    print!(" {x:02x}");
                }
                println!();
                // Interpret as bit array: count set bits
                let mut set = 0usize;
                for byte in b {
                    set += byte.count_ones() as usize;
                }
                println!("  set_bits={set} / {}", b.len() * 8);
                // Print as bit rows if gx*gy fits
                let cells = (gx * gy) as usize;
                if cells > 0 && cells <= b.len() * 8 {
                    println!("  bitmask {gx}x{gy} (treating bits row-major):");
                    for y in 0..gy {
                        let mut row = String::new();
                        for x in 0..gx {
                            let i = (y * gx + x) as usize;
                            let bit = (b[i / 8] >> (i % 8)) & 1;
                            row.push(if bit != 0 { '#' } else { '.' });
                        }
                        println!("  {row}");
                    }
                }
            }
        }
    }
}
