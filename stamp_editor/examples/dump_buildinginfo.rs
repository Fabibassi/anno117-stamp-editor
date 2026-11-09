use stamp_editor::filedb::Node;
use stamp_editor::pack::load_pack;
use stamp_editor::stamp::decode_pipette_b64;
use std::path::PathBuf;

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
    println!("buildings extracted: {}", stamp.buildings.len());
    for root in &stamp.document.roots {
        let Node::Tag(t) = root else { continue };
        if t.name != "PipetteData" {
            continue;
        }
        for c in &t.children {
            let Node::Tag(ch) = c else { continue };
            println!("-- child {} ({} nodes)", ch.name, ch.children.len());
            if ch.name == "BuildingInfo" {
                let mut n = 0;
                for e in &ch.children {
                    let Node::Tag(entry) = e else { continue };
                    if entry.name != "None" {
                        println!("  non-None child: {}", entry.name);
                        continue;
                    }
                    n += 1;
                    let mut guid = 0u32;
                    let mut has_pos = false;
                    let mut names = Vec::new();
                    for a in &entry.children {
                        match a {
                            Node::Attrib(at) => {
                                names.push(at.name.clone());
                                if at.name == "GUID" && at.content.len() >= 4 {
                                    guid = u32::from_le_bytes(
                                        at.content[0..4].try_into().unwrap(),
                                    );
                                }
                                if at.name == "Pos" {
                                    has_pos = true;
                                }
                            }
                            Node::Tag(tg) => names.push(format!("tag:{}", tg.name)),
                        }
                    }
                    println!("  entry{n}: guid={guid} has_pos={has_pos} fields={names:?}");
                }
                println!("total None entries: {n}");
            }
            if ch.name == "Polygons" {
                for e in &ch.children {
                    if let Node::Tag(p) = e {
                        println!(
                            "  polygon tag={} children={}",
                            p.name,
                            p.children.len()
                        );
                    }
                }
            }
        }
    }
}
