//! Empirically dump Pos written for a cistern placed flush at TL=(1,0)
//! vs a centered neighbor. Run: `cargo run --example probe_cistern_pos`
use stamp_editor::catalog::{blocker_min_for, size_for_guid};
use stamp_editor::coverage::footprint_cells;
use stamp_editor::stamp::{decode_pipette_b64, encode_pipette_b64, Stamp};

fn main() {
    let guid = 19753u32;
    let (w, h) = size_for_guid(guid, 3.0, 3.0, None, None);
    let tl = (1i32, 0i32); // flush to road at x=0
    let (mx, my) = blocker_min_for(guid, w, h, None, None);
    let old_centered = (tl.0 as f32 + w * 0.5, tl.1 as f32 + h * 0.5);
    let new_pos = (tl.0 as f32 - mx, tl.1 as f32 - my);

    println!("=== Cistern TL=({},{}); forced size={}x{} ===", tl.0, tl.1, w, h);
    println!("blocker_min:            ({:.3}, {:.3})", mx, my);
    println!("OLD centered Pos:       ({:.3}, {:.3})  → game TL≈{:.0} (+1 gap)", old_centered.0, old_centered.1, old_centered.0 + mx);
    println!("NEW pivot Pos:          ({:.3}, {:.3})  → game TL≈{:.0} (flush)", new_pos.0, new_pos.1, new_pos.0 + mx);

    let mut stamp = Stamp::blank().unwrap();
    stamp
        .add_building_from_template(guid, new_pos.0, new_pos.1, 0, None)
        .unwrap();
    stamp
        .add_building_from_template(2878, old_centered.0, tl.1 as f32 + 4.5, 0, None)
        .unwrap();
    let enc = encode_pipette_b64(&stamp).unwrap();
    let back = decode_pipette_b64(&enc).unwrap();
    println!("\n=== Encoded BuildingInfo Pos ===");
    for b in &back.buildings {
        let cells = footprint_cells(b, None);
        let min_x = cells.iter().map(|c| c.x).min().unwrap();
        println!(
            "  GUID {} Pos=({:.4}, {:.4}) editor_TL.x={} {}",
            b.guid,
            b.x,
            b.y,
            min_x,
            if b.guid == guid {
                if min_x == 1 {
                    "FLUSH ✓"
                } else {
                    "NOT FLUSH"
                }
            } else {
                ""
            }
        );
    }
}
