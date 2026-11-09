use stamp_editor::filedb::{Attrib, Node, Tag};
use stamp_editor::pack::load_pack;
use stamp_editor::stamp::decode_pipette_b64;
use std::path::PathBuf;

fn dump_attrib(pad: &str, a: &Attrib) {
    if a.content.len() == 8 {
        let x = f32::from_le_bytes(a.content[0..4].try_into().unwrap());
        let y = f32::from_le_bytes(a.content[4..8].try_into().unwrap());
        println!("{pad}attrib {} = ({x}, {y})", a.name);
    } else if a.content.len() == 4 {
        let i = i32::from_le_bytes(a.content[0..4].try_into().unwrap());
        println!("{pad}attrib {} = {i}", a.name);
    } else if a.content.len() >= 8 && a.content.len() % 4 == 0 {
        let vals: Vec<String> = a
            .content
            .chunks_exact(4)
            .take(16)
            .map(|c| format!("{:.2}", f32::from_le_bytes(c.try_into().unwrap())))
            .collect();
        println!(
            "{pad}attrib {} f32s=[{}] n={}",
            a.name,
            vals.join(","),
            a.content.len() / 4
        );
    } else {
        println!("{pad}attrib {} len={}", a.name, a.content.len());
    }
}

fn dump_tag(tag: &Tag, indent: usize) {
    let pad = " ".repeat(indent);
    println!("{pad}tag {}", tag.name);
    for c in &tag.children {
        match c {
            Node::Attrib(a) => dump_attrib(&pad, a),
            Node::Tag(t) => dump_tag(t, indent + 2),
        }
    }
}

fn main() {
    let pack = load_pack(&PathBuf::from(
        r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\Region Roman\latium.json",
    ))
    .unwrap();
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Loungers x4".into());
    let entry = pack.stamps.iter().find(|s| s.name == name).unwrap();
    let stamp = decode_pipette_b64(&entry.pipette_data).unwrap();
    for root in &stamp.document.roots {
        let Node::Tag(t) = root else { continue };
        if t.name != "PipetteData" {
            continue;
        }
        for c in &t.children {
            let Node::Tag(ch) = c else { continue };
            if ch.name != "Polygons" {
                continue;
            }
            for (i, e) in ch.children.iter().enumerate() {
                if let Node::Tag(p) = e {
                    println!("=== polygon {i} ===");
                    dump_tag(p, 0);
                }
            }
        }
    }
}
