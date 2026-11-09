//! Anno / BlueByte FileDB (BBDom) V2/V3 reader & writer.

use std::collections::BTreeMap;
use thiserror::Error;

pub const NONE_TAG_ID: u16 = 1;
pub const NONE_ATTRIB_ID: u16 = 0x8000;
pub const BLOCK_SIZE: usize = 8;
pub const MAGIC_V2: [u8; 8] = [0x08, 0x00, 0x00, 0x00, 0xFE, 0xFF, 0xFF, 0xFF];
pub const MAGIC_V3: [u8; 8] = [0x08, 0x00, 0x00, 0x00, 0xFD, 0xFF, 0xFF, 0xFF];

#[derive(Debug, Error)]
pub enum FileDbError {
    #[error("unknown FileDB version")]
    UnknownVersion,
    #[error("invalid FileDB structure: {0}")]
    Invalid(&'static str),
    #[error("I/O cursor out of range")]
    OutOfRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    V2,
    V3,
}

#[derive(Debug, Clone)]
pub enum Node {
    Attrib(Attrib),
    Tag(Tag),
}

#[derive(Debug, Clone)]
pub struct Attrib {
    pub id: u16,
    pub name: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Tag {
    pub id: u16,
    pub name: String,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub version: Version,
    pub roots: Vec<Node>,
    pub tags: BTreeMap<u16, String>,
    pub attribs: BTreeMap<u16, String>,
}

impl Document {
    pub fn tag_name(&self, id: u16) -> String {
        if id == NONE_TAG_ID {
            return "None".into();
        }
        self.tags
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("t_{id}"))
    }

    pub fn attrib_name(&self, id: u16) -> String {
        if id == NONE_ATTRIB_ID {
            return "None".into();
        }
        self.attribs
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("a_{id}"))
    }

    pub fn count_nodes(&self) -> i32 {
        fn walk(nodes: &[Node]) -> i32 {
            let mut n = 0;
            for node in nodes {
                n += 1;
                if let Node::Tag(t) = node {
                    n += walk(&t.children);
                }
            }
            n
        }
        1 + walk(&self.roots)
    }
}

pub fn block_space(size: usize) -> usize {
    if size == 0 {
        return 0;
    }
    let rem = size % BLOCK_SIZE;
    if rem == 0 {
        size
    } else {
        size + (BLOCK_SIZE - rem)
    }
}

fn detect_version(data: &[u8]) -> Result<Version, FileDbError> {
    if data.ends_with(&MAGIC_V3) {
        Ok(Version::V3)
    } else if data.ends_with(&MAGIC_V2) {
        Ok(Version::V2)
    } else {
        Err(FileDbError::UnknownVersion)
    }
}

fn read_cstring(data: &[u8], mut offset: usize) -> Result<(String, usize), FileDbError> {
    let start = offset;
    while offset < data.len() && data[offset] != 0 {
        offset += 1;
    }
    if offset >= data.len() {
        return Err(FileDbError::OutOfRange);
    }
    let s = String::from_utf8_lossy(&data[start..offset]).into_owned();
    Ok((s, offset + 1))
}

fn parse_dictionary(data: &[u8], mut offset: usize) -> Result<BTreeMap<u16, String>, FileDbError> {
    if offset + 4 > data.len() {
        return Err(FileDbError::OutOfRange);
    }
    let count = i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
    offset += 4;
    if offset + 2 * count > data.len() {
        return Err(FileDbError::OutOfRange);
    }
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        let id = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
        offset += 2;
        ids.push(id);
    }
    let mut map = BTreeMap::new();
    for id in ids {
        let (name, next) = read_cstring(data, offset)?;
        offset = next;
        map.insert(id, name);
    }
    // Preserve insertion order via BTreeMap is sorted by id - PROBLEM!
    // Original FileDB dictionaries are ordered by appearance, not by id.
    // For round-trip we must preserve order. Use IndexMap or Vec.
    let _ = offset;
    Ok(map)
}

/// Ordered dictionary (insertion order matters for byte-identical writes).
#[derive(Debug, Clone, Default)]
pub struct OrderedDict {
    pub items: Vec<(u16, String)>,
}

impl OrderedDict {
    pub fn get(&self, id: u16) -> Option<&str> {
        self.items
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, s)| s.as_str())
    }

    pub fn to_map(&self) -> BTreeMap<u16, String> {
        self.items.iter().cloned().collect()
    }
}

fn parse_dictionary_ordered(data: &[u8], mut offset: usize) -> Result<OrderedDict, FileDbError> {
    if offset + 4 > data.len() {
        return Err(FileDbError::OutOfRange);
    }
    let count = i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
    offset += 4;
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        if offset + 2 > data.len() {
            return Err(FileDbError::OutOfRange);
        }
        ids.push(u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()));
        offset += 2;
    }
    let mut items = Vec::with_capacity(count);
    for id in ids {
        let (name, next) = read_cstring(data, offset)?;
        offset = next;
        items.push((id, name));
    }
    Ok(OrderedDict { items })
}

#[derive(Debug, Clone)]
pub struct DocumentOrdered {
    pub version: Version,
    pub roots: Vec<Node>,
    pub tags: OrderedDict,
    pub attribs: OrderedDict,
}

impl DocumentOrdered {
    pub fn tag_name(&self, id: u16) -> String {
        if id == NONE_TAG_ID {
            return "None".into();
        }
        self.tags
            .get(id)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("t_{id}"))
    }

    pub fn attrib_name(&self, id: u16) -> String {
        if id == NONE_ATTRIB_ID {
            return "None".into();
        }
        self.attribs
            .get(id)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("a_{id}"))
    }

    pub fn count_nodes(&self) -> i32 {
        fn walk(nodes: &[Node]) -> i32 {
            let mut n = 0;
            for node in nodes {
                n += 1;
                if let Node::Tag(t) = node {
                    n += walk(&t.children);
                }
            }
            n
        }
        1 + walk(&self.roots)
    }
}

pub fn load(data: &[u8]) -> Result<DocumentOrdered, FileDbError> {
    let version = detect_version(data)?;
    if data.len() < 16 {
        return Err(FileDbError::Invalid("too short"));
    }
    let tags_offset = i32::from_le_bytes(data[data.len() - 16..data.len() - 12].try_into().unwrap()) as usize;
    let attribs_offset =
        i32::from_le_bytes(data[data.len() - 12..data.len() - 8].try_into().unwrap()) as usize;
    let tags = parse_dictionary_ordered(data, tags_offset)?;
    let attribs = parse_dictionary_ordered(data, attribs_offset)?;

    let mut doc = DocumentOrdered {
        version,
        roots: Vec::new(),
        tags,
        attribs,
    };

    let content_end = tags_offset;
    let mut pos = 0usize;
    let mut tag_stack: Vec<Tag> = Vec::new();
    let mut roots: Vec<Node> = Vec::new();

    while pos + 8 <= content_end {
        let bytesize = i32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        let raw_id = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap());
        pos += 8;
        let node_id = (raw_id & 0xFFFF) as u16;

        if node_id == 0 {
            if tag_stack.is_empty() {
                break;
            }
            let finished = tag_stack.pop().unwrap();
            if let Some(parent) = tag_stack.last_mut() {
                parent.children.push(Node::Tag(finished));
            } else {
                roots.push(Node::Tag(finished));
            }
            continue;
        }

        if node_id & 0x8000 != 0 {
            let size = bytesize as usize;
            let space = block_space(size);
            if pos + space > content_end {
                return Err(FileDbError::Invalid("attrib overflow"));
            }
            let content = data[pos..pos + size].to_vec();
            pos += space;
            let attrib = Attrib {
                id: node_id,
                name: doc.attrib_name(node_id),
                content,
            };
            if let Some(parent) = tag_stack.last_mut() {
                parent.children.push(Node::Attrib(attrib));
            } else {
                roots.push(Node::Attrib(attrib));
            }
        } else {
            tag_stack.push(Tag {
                id: node_id,
                name: doc.tag_name(node_id),
                children: Vec::new(),
            });
        }
    }

    doc.roots = roots;
    Ok(doc)
}

fn write_dictionary(buf: &mut Vec<u8>, dict: &OrderedDict) -> i32 {
    let start = buf.len() as i32;
    buf.extend_from_slice(&(dict.items.len() as i32).to_le_bytes());
    for (id, _) in &dict.items {
        buf.extend_from_slice(&id.to_le_bytes());
    }
    for (_, name) in &dict.items {
        buf.extend_from_slice(name.as_bytes());
        buf.push(0);
    }
    let written = buf.len() - start as usize;
    let pad = block_space(written) - written;
    buf.extend(std::iter::repeat_n(0u8, pad));
    start
}

fn write_node(buf: &mut Vec<u8>, node: &Node) {
    match node {
        Node::Attrib(a) => {
            buf.extend_from_slice(&(a.content.len() as i32).to_le_bytes());
            buf.extend_from_slice(&(a.id as u32).to_le_bytes());
            buf.extend_from_slice(&a.content);
            let pad = block_space(a.content.len()) - a.content.len();
            buf.extend(std::iter::repeat_n(0u8, pad));
        }
        Node::Tag(t) => {
            buf.extend_from_slice(&0i32.to_le_bytes());
            buf.extend_from_slice(&(t.id as u32).to_le_bytes());
            for child in &t.children {
                write_node(buf, child);
            }
            buf.extend_from_slice(&0i64.to_le_bytes());
        }
    }
}

pub fn dump(doc: &DocumentOrdered) -> Vec<u8> {
    let mut buf = Vec::new();
    for root in &doc.roots {
        write_node(&mut buf, root);
    }
    buf.extend_from_slice(&0i64.to_le_bytes());

    let tags_offset = write_dictionary(&mut buf, &doc.tags);
    let attribs_offset = write_dictionary(&mut buf, &doc.attribs);

    if doc.version == Version::V3 {
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&doc.count_nodes().to_le_bytes());
    }
    buf.extend_from_slice(&tags_offset.to_le_bytes());
    buf.extend_from_slice(&attribs_offset.to_le_bytes());
    match doc.version {
        Version::V3 => buf.extend_from_slice(&MAGIC_V3),
        Version::V2 => buf.extend_from_slice(&MAGIC_V2),
    }
    buf
}

// Keep parse_dictionary referenced for now via allow
#[allow(dead_code)]
fn _unused() {
    let _ = parse_dictionary;
}

