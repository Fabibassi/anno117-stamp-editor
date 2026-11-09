use stamp_editor::filedb::Node;
use stamp_editor::pack::load_pack;
use stamp_editor::stamp::decode_pipette_b64;
use std::path::PathBuf;

fn main() {
    let pack = load_pack(&PathBuf::from(
        r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\Region Roman\latium.json",
    ))
    .unwrap();
    for name in ["Pileus x8", "Loungers x4", "Bread x4", "Soap x4"] {
        let Some(e) = pack.stamps.iter().find(|s| s.name == name) else {
            println!("{name}: missing");
            continue;
        };
        let st = decode_pipette_b64(&e.pipette_data).unwrap();
        let mut poly = 0usize;
        for root in &st.document.roots {
            let Node::Tag(t) = root else { continue };
            if t.name != "PipetteData" {
                continue;
            }
            for c in &t.children {
                let Node::Tag(ch) = c else { continue };
                if ch.name == "Polygons" {
                    poly = ch
                        .children
                        .iter()
                        .filter(|n| matches!(n, Node::Tag(t) if t.name == "None"))
                        .count();
                }
            }
        }
        let mods = st
            .buildings
            .iter()
            .filter(|b| {
                matches!(
                    b.guid,
                    2787 | 8987 | 2682 | 2732 | 2744 | 2788 | 2682
                )
            })
            .count();
        println!(
            "{name}: buildings={} polygons={} bi_modules≈{}",
            st.buildings.len(),
            poly,
            mods
        );
    }
}
