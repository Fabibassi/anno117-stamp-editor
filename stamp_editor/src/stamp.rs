use crate::filedb::{dump, load, Attrib, DocumentOrdered, Node, Tag};
use crate::meshgraph::{
    clear_roads, clone_polygon_for_new_building, recompute_bounds, sync_after_building_move,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::{Read, Write};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StampError {
    #[error(transparent)]
    FileDb(#[from] crate::filedb::FileDbError),
    #[error("zlib error: {0}")]
    Zlib(String),
    #[error("missing building field")]
    MissingField,
}

#[derive(Debug, Clone)]
pub struct Building {
    pub guid: u32,
    pub x: f32,
    pub y: f32,
    pub direction: i32,
    pub complex_owner_id: Option<u64>,
    /// Pipette `Variation` index (e.g. Warehouse 7055: 1 = marsh CFG 5×5).
    pub variation: Option<u32>,
}

/// One occupied tile from a `Polygons` / SubTileGrid farm field.
#[derive(Debug, Clone, Copy)]
pub struct PolygonCell {
    pub guid: u32,
    pub x: i32,
    pub y: i32,
    pub complex_owner_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Stamp {
    pub document: DocumentOrdered,
    pub buildings: Vec<Building>,
    /// Field/pasture modules stored under `Polygons` (GUID + RelativePos), not BuildingInfo.
    /// Fallback when a polygon has no SubTileGrid bits.
    pub polygon_modules: Vec<Building>,
    /// Occupied tiles decoded from SubTileGrid bitmasks (fills stamp road wings).
    pub polygon_cells: Vec<PolygonCell>,
    /// PlantFarm area toward ModuleLimits: ComplexOwnerID → `set_bits / 4`
    /// (4 horizontal quarter-bits = 1 tile). Prefer over `polygon_cells.len()` for progress.
    pub polygon_area: std::collections::HashMap<u64, u32>,
    pub stamp_version: i32,
}

fn u32_le(data: &[u8]) -> u32 {
    if data.len() < 4 {
        return 0;
    }
    u32::from_le_bytes(data[0..4].try_into().unwrap())
}

fn i32_le(data: &[u8]) -> i32 {
    if data.len() < 4 {
        return 0;
    }
    i32::from_le_bytes(data[0..4].try_into().unwrap())
}

fn f32x2(data: &[u8]) -> Option<(f32, f32)> {
    if data.len() < 8 {
        return None;
    }
    Some((
        f32::from_le_bytes(data[0..4].try_into().unwrap()),
        f32::from_le_bytes(data[4..8].try_into().unwrap()),
    ))
}

fn find_tag<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Tag> {
    nodes.iter().find_map(|n| match n {
        Node::Tag(t) if t.name == name => Some(t),
        _ => None,
    })
}

fn find_attrib<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Attrib> {
    nodes.iter().find_map(|n| match n {
        Node::Attrib(a) if a.name == name => Some(a),
        _ => None,
    })
}

fn find_tag_mut<'a>(nodes: &'a mut [Node], name: &str) -> Option<&'a mut Tag> {
    nodes.iter_mut().find_map(|n| match n {
        Node::Tag(t) if t.name == name => Some(t),
        _ => None,
    })
}

fn find_attrib_mut<'a>(nodes: &'a mut [Node], name: &str) -> Option<&'a mut Attrib> {
    nodes.iter_mut().find_map(|n| match n {
        Node::Attrib(a) if a.name == name => Some(a),
        _ => None,
    })
}

fn read_direction(building: &Tag) -> i32 {
    let Some(dir_tag) = find_tag(&building.children, "Dir") else {
        return 0;
    };
    let Some(value) = find_tag(&dir_tag.children, "value") else {
        return 0;
    };
    let Some(none1) = find_tag(&value.children, "None") else {
        return 0;
    };
    let Some(attrib) = find_attrib(&none1.children, "None") else {
        return 0;
    };
    i32_le(&attrib.content)
}

fn write_direction(entry: &mut Tag, direction: i32) -> bool {
    let Some(dir_tag) = entry.children.iter_mut().find_map(|n| match n {
        Node::Tag(t) if t.name == "Dir" => Some(t),
        _ => None,
    }) else {
        return false;
    };
    let Some(value) = dir_tag.children.iter_mut().find_map(|n| match n {
        Node::Tag(t) if t.name == "value" => Some(t),
        _ => None,
    }) else {
        return false;
    };
    let Some(none1) = value.children.iter_mut().find_map(|n| match n {
        Node::Tag(t) if t.name == "None" => Some(t),
        _ => None,
    }) else {
        return false;
    };
    let Some(attrib) = find_attrib_mut(&mut none1.children, "None") else {
        return false;
    };
    attrib.content = direction.to_le_bytes().to_vec();
    true
}

fn owner_id_from_attrib(data: &[u8]) -> Option<u64> {
    if data.len() >= 8 {
        Some(u64::from_le_bytes(data[0..8].try_into().unwrap()))
    } else if data.len() >= 4 {
        Some(u32_le(data) as u64)
    } else {
        None
    }
}

fn extract_buildings(doc: &DocumentOrdered) -> Vec<Building> {
    let mut out = Vec::new();
    for root in &doc.roots {
        let Node::Tag(pipette) = root else { continue };
        if pipette.name != "PipetteData" {
            continue;
        }
        let Some(info) = find_tag(&pipette.children, "BuildingInfo") else {
            continue;
        };
        for child in &info.children {
            let Node::Tag(entry) = child else { continue };
            if entry.name != "None" {
                continue;
            }
            let Some(guid_a) = find_attrib(&entry.children, "GUID") else {
                continue;
            };
            // Some authored stamps omit Pos on the center warehouse (GUID 3310);
            // the game treats that as DragRectCenter-relative (0,0). Skipping them
            // hid the WH and desynced BuildingInfo indices vs `buildings[]`.
            let pos = find_attrib(&entry.children, "Pos")
                .and_then(|a| f32x2(&a.content))
                .unwrap_or((0.0, 0.0));
            let owner = find_attrib(&entry.children, "ComplexOwnerID")
                .and_then(|a| owner_id_from_attrib(&a.content));
            let variation = find_attrib(&entry.children, "Variation").and_then(|a| {
                if a.content.len() >= 4 {
                    Some(u32_le(&a.content))
                } else {
                    None
                }
            });
            out.push(Building {
                guid: u32_le(&guid_a.content),
                x: pos.0,
                y: pos.1,
                direction: read_direction(entry),
                complex_owner_id: owner,
                variation,
            });
        }
    }
    out
}

fn drag_center(doc: &DocumentOrdered) -> (f32, f32) {
    for root in &doc.roots {
        let Node::Tag(pipette) = root else { continue };
        if pipette.name != "PipetteData" {
            continue;
        }
        if let Some(mesh) = find_tag(&pipette.children, "MeshGraphData") {
            if let Some(p) = find_attrib(&mesh.children, "DragRectCenter").and_then(|a| f32x2(&a.content))
            {
                return p;
            }
        }
        if let Some(p) =
            find_attrib(&pipette.children, "DragRectCenter").and_then(|a| f32x2(&a.content))
        {
            return p;
        }
    }
    (0.0, 0.0)
}

fn bit_set(bits: &[u8], index: usize) -> bool {
    let byte = index / 8;
    let bit = index % 8;
    bits.get(byte).map(|b| (b >> bit) & 1 == 1).unwrap_or(false)
}

/// Decode SubTileGrid → unique tile cells + set-bit count.
/// `set_bits / 4` is the PlantFarm area unit (2×2 half-tile bits per tile).
fn subtile_grid_cells(
    entry: &Tag,
    drag: (f32, f32),
    guid: u32,
    complex_owner_id: Option<u64>,
) -> (Vec<PolygonCell>, u32) {
    let Some(stg) = find_tag(&entry.children, "SubTileGrid") else {
        return (Vec::new(), 0);
    };
    let Some(origin_a) = find_attrib(&stg.children, "GridOriginWS") else {
        return (Vec::new(), 0);
    };
    if origin_a.content.len() < 8 {
        return (Vec::new(), 0);
    }
    let ox_abs = i32_le(&origin_a.content[0..4]);
    let oy_abs = i32_le(&origin_a.content[4..8]);
    let Some(grid_wrap) = find_tag(&stg.children, "Grid") else {
        return (Vec::new(), 0);
    };
    let Some(grid) = find_tag(&grid_wrap.children, "grid") else {
        return (Vec::new(), 0);
    };
    let gx = find_attrib(&grid.children, "x")
        .map(|a| i32_le(&a.content))
        .unwrap_or(0)
        .max(0) as usize;
    let gy = find_attrib(&grid.children, "y")
        .map(|a| i32_le(&a.content))
        .unwrap_or(0)
        .max(0) as usize;
    let Some(bits_a) = find_attrib(&grid.children, "bits") else {
        return (Vec::new(), 0);
    };
    if gx == 0 || gy == 0 {
        return (Vec::new(), 0);
    }
    // SubTileGrid bit layout (Anno 117 PlantFarm):
    //   - each ROW = one world tile in Y
    //   - every 4 COLUMNS = one world tile in X (horizontal quarter-bits)
    // Isotropic 0.25 collapsed height (~3–4 tiles); isotropic 0.5 stretched X past the
    // road frame. Area unit remains set_bits/4. Rows pad to 32-bit stride.
    let stride = ((gx + 31) / 32) * 32;
    let mut cells = std::collections::BTreeSet::new();
    let mut set_bits = 0u32;
    for sy in 0..gy {
        for sx in 0..gx {
            let idx = sy * stride + sx;
            if !bit_set(&bits_a.content, idx) {
                continue;
            }
            set_bits += 1;
            let ax = ox_abs + (sx as i32) / 4;
            let ay = oy_abs + sy as i32;
            let tx = ax - drag.0.floor() as i32;
            let ty = ay - drag.1.floor() as i32;
            cells.insert((tx, ty));
        }
    }
    let cells = cells
        .into_iter()
        .map(|(x, y)| PolygonCell {
            guid,
            x,
            y,
            complex_owner_id,
        })
        .collect();
    (cells, set_bits)
}

/// Farm modules that live in `Polygons` (RelativePos + GUID), common in production stamps.
fn extract_polygon_modules(
    doc: &DocumentOrdered,
) -> (
    Vec<Building>,
    Vec<PolygonCell>,
    std::collections::HashMap<u64, u32>,
) {
    let drag = drag_center(doc);
    let mut modules = Vec::new();
    let mut cells = Vec::new();
    let mut area: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();
    for root in &doc.roots {
        let Node::Tag(pipette) = root else { continue };
        if pipette.name != "PipetteData" {
            continue;
        }
        let Some(polys) = find_tag(&pipette.children, "Polygons") else {
            continue;
        };
        for child in &polys.children {
            let Node::Tag(entry) = child else { continue };
            if entry.name != "None" {
                continue;
            }
            let Some(guid_a) = find_attrib(&entry.children, "GUID") else {
                continue;
            };
            let guid = u32_le(&guid_a.content);
            if guid == 0 {
                continue;
            }
            let owner = find_attrib(&entry.children, "ComplexOwnerID")
                .and_then(|a| owner_id_from_attrib(&a.content));
            let (grid_cells, set_bits) = subtile_grid_cells(entry, drag, guid, owner);
            if let Some(own) = owner.filter(|o| *o != 0) {
                *area.entry(own).or_default() += set_bits / 4;
            }
            // Module marker (icon + ComplexOwner link) even when SubTileGrid fills cells.
            let pos = find_attrib(&entry.children, "RelativePos")
                .and_then(|a| f32x2(&a.content))
                .or_else(|| {
                    if grid_cells.is_empty() {
                        None
                    } else {
                        let n = grid_cells.len() as f32;
                        let cx = grid_cells.iter().map(|c| c.x as f32).sum::<f32>() / n + 0.5;
                        let cy = grid_cells.iter().map(|c| c.y as f32).sum::<f32>() / n + 0.5;
                        Some((cx, cy))
                    }
                });
            if !grid_cells.is_empty() {
                cells.extend(grid_cells);
            }
            let Some(pos) = pos else {
                continue;
            };
            modules.push(Building {
                guid,
                x: pos.0,
                y: pos.1,
                direction: 0,
                complex_owner_id: owner,
                variation: None,
            });
        }
    }
    (modules, cells, area)
}

pub fn decode_pipette_b64(data: &str) -> Result<Stamp, StampError> {
    let compressed = B64.decode(data).map_err(|e| StampError::Zlib(e.to_string()))?;
    let mut dec = ZlibDecoder::new(std::io::Cursor::new(compressed));
    let mut raw = Vec::new();
    dec.read_to_end(&mut raw)
        .map_err(|e| StampError::Zlib(e.to_string()))?;
    decode_pipette_bytes(&raw)
}

pub fn decode_pipette_bytes(raw: &[u8]) -> Result<Stamp, StampError> {
    let document = load(raw)?;
    let stamp_version = document
        .roots
        .iter()
        .find_map(|n| match n {
            Node::Tag(t) if t.name == "PipetteData" => find_attrib(&t.children, "StampVersion"),
            _ => None,
        })
        .map(|a| i32_le(&a.content))
        .unwrap_or(1);
    let buildings = extract_buildings(&document);
    let (polygon_modules, polygon_cells, polygon_area) = extract_polygon_modules(&document);
    let polygon_cells = filter_field_cells_off_roads(&document, polygon_cells);
    Ok(Stamp {
        document,
        buildings,
        polygon_modules,
        polygon_cells,
        polygon_area,
        stamp_version,
    })
}

/// Half-tile bits can touch the road frame; never paint fields on streets.
fn filter_field_cells_off_roads(
    document: &DocumentOrdered,
    mut cells: Vec<PolygonCell>,
) -> Vec<PolygonCell> {
    let roads: std::collections::HashSet<(i32, i32)> =
        crate::meshgraph::mesh_road_cells(document)
            .into_iter()
            .map(|(x, y, _)| (x, y))
            .collect();
    if !roads.is_empty() {
        cells.retain(|c| !roads.contains(&(c.x, c.y)));
    }
    cells
}

pub fn encode_pipette_b64(stamp: &Stamp) -> Result<String, StampError> {
    let raw = dump(&stamp.document);
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
    enc.write_all(&raw)
        .map_err(|e| StampError::Zlib(e.to_string()))?;
    let compressed = enc.finish().map_err(|e| StampError::Zlib(e.to_string()))?;
    Ok(B64.encode(compressed))
}

/// Minimal valid pipette (from a tiny in-game stamp) used as document skeleton /
/// building-entry template when creating a blank stamp.
const BLANK_SEED_PIPETTE_B64: &str = "eNrFVU1OwlAQHqGlpcVEjmDiATyBKVBCWBCNojsXjTxLk9KStiju3tYlJ4CdS70ASW/h0ht4A4Pz2mKbBkL5k0km7TdvvvdN30zzAABy6Bz6EUWH2PLRk8VElkcZmvgAvg8RhxmfyAtiNF5bZmw9j3kXcojZ/hxdnJvSV7LoF/amz2y1fi6jfrm0rn6ovUof9qYf9F/57/7vqt/fpY3mTTnwvCkHnDcly7xxO9Sf2/xMhSifT52zkKqjgOv3p/H+AuLXE4D3cohFxKMIp793GX+U4o834E9T/Oka/HFK/23L+kdb6i+qP92PXeH53BRR9yzCKA8SYhUL+kRvo3+Jcc1JKybe2ZrM5s3n6vBxXE/mSRnz5nty+LcJeDvyOI0iRiRQDQeujD7xPKJqngb1jk5cqA4Ms2NYetN6tOFJMwcEWsTtNhyt3w3SVGISj1yTBw+aumU7JORd2eaLblsu05ODezlH87RAOcpTgYq0SCUq0xtP6/XviOMatoUUFxq3TRX0gdGBmt3rm2R4+WwRB2N/ojV7YHkst2UElJY2hLZhkjBeqVSroDqazgqqEcsjTvjNrJBzbM5ECM/nZzab/QKoeplWeNoDAAAAAAE4CAAA";

fn seed_building_template_tag() -> Result<Tag, StampError> {
    let seed = decode_pipette_b64(BLANK_SEED_PIPETTE_B64)?;
    for root in &seed.document.roots {
        let Node::Tag(pipette) = root else { continue };
        if pipette.name != "PipetteData" {
            continue;
        }
        let Some(info) = find_tag(&pipette.children, "BuildingInfo") else {
            continue;
        };
        for child in &info.children {
            if let Node::Tag(entry) = child {
                if entry.name == "None" {
                    return Ok(entry.clone());
                }
            }
        }
    }
    Err(StampError::MissingField)
}

impl Stamp {
    /// Empty editable stamp (valid pipette shell, zero buildings).
    pub fn blank() -> Result<Self, StampError> {
        let mut stamp = decode_pipette_b64(BLANK_SEED_PIPETTE_B64)?;
        for root in &mut stamp.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            if let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") {
                info.children.retain(|n| !matches!(n, Node::Tag(t) if t.name == "None"));
            }
            if let Some(polygons) = find_tag_mut(&mut pipette.children, "Polygons") {
                polygons.children.retain(|n| !matches!(n, Node::Tag(t) if t.name == "None"));
            }
        }
        stamp.buildings.clear();
        stamp.polygon_modules.clear();
        stamp.polygon_cells.clear();
        stamp.polygon_area.clear();
        // Seed pipette still carries MeshGraph streets — wipe them for a true blank.
        let _ = clear_roads(&mut stamp.document, &stamp.buildings);
        recompute_bounds(&mut stamp.document, &stamp.buildings);
        Ok(stamp)
    }

    pub fn refresh_buildings(&mut self) {
        self.buildings = extract_buildings(&self.document);
        let (mods, cells, area) = extract_polygon_modules(&self.document);
        self.polygon_modules = mods;
        self.polygon_cells = filter_field_cells_off_roads(&self.document, cells);
        self.polygon_area = area;
    }

    /// BuildingInfo entries plus polygon-stored field modules (for paint / occupancy).
    pub fn all_draw_buildings(&self) -> impl Iterator<Item = &Building> {
        self.buildings.iter().chain(self.polygon_modules.iter())
    }

    pub fn set_building_pos(&mut self, index: usize, x: f32, y: f32) -> Result<(), StampError> {
        self.set_building_pos_ex(index, x, y, false)
    }

    pub fn set_building_direction(&mut self, index: usize, direction: i32) -> Result<(), StampError> {
        let direction = direction.rem_euclid(8);
        let mut count = 0usize;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                if count == index {
                    if !write_direction(entry, direction) {
                        return Err(StampError::MissingField);
                    }
                    if let Some(b) = self.buildings.get_mut(index) {
                        b.direction = direction;
                    }
                    return Ok(());
                }
                count += 1;
            }
        }
        Err(StampError::MissingField)
    }

    /// Move building. When `move_linked` is true, also shift modules/polygons with same ComplexOwnerID.
    pub fn set_building_pos_ex(
        &mut self,
        index: usize,
        x: f32,
        y: f32,
        move_linked: bool,
    ) -> Result<(), StampError> {
        let old = self
            .buildings
            .get(index)
            .map(|b| (b.x, b.y))
            .ok_or(StampError::MissingField)?;
        let mut count = 0usize;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                if count == index {
                    // Create Pos when absent (center-warehouse stamps omit it).
                    let pos = if let Some(p) = find_attrib_mut(&mut entry.children, "Pos") {
                        p
                    } else {
                        entry.children.push(Node::Attrib(Attrib {
                            id: 0,
                            name: "Pos".into(),
                            content: vec![0u8; 8],
                        }));
                        find_attrib_mut(&mut entry.children, "Pos")
                            .ok_or(StampError::MissingField)?
                    };
                    let mut bytes = vec![0u8; 8];
                    bytes[0..4].copy_from_slice(&x.to_le_bytes());
                    bytes[4..8].copy_from_slice(&y.to_le_bytes());
                    pos.content = bytes;
                    if let Some(b) = self.buildings.get_mut(index) {
                        b.x = x;
                        b.y = y;
                    }
                    if move_linked {
                        sync_after_building_move(
                            &mut self.document,
                            &mut self.buildings,
                            index,
                            old.0,
                            old.1,
                            x,
                            y,
                        );
                    } else {
                        recompute_bounds(&mut self.document, &self.buildings);
                    }
                    // Rebuild polygon_modules / SubTileGrid cells from the document.
                    self.refresh_buildings();
                    return Ok(());
                }
                count += 1;
            }
        }
        Err(StampError::MissingField)
    }

    pub fn remove_building(&mut self, index: usize) -> Result<(), StampError> {
        let owner = self
            .buildings
            .get(index)
            .and_then(|b| b.complex_owner_id)
            .filter(|o| *o != 0);
        let guid = self
            .buildings
            .get(index)
            .map(|b| b.guid)
            .ok_or(StampError::MissingField)?;
        // If this was the last BuildingInfo with that ComplexOwnerID, drop owned field polygons.
        let drop_owned_polygons = owner.is_some_and(|own| {
            self.buildings
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != index)
                .filter(|(_, b)| b.complex_owner_id == Some(own))
                .count()
                == 0
        });
        let mut count = 0usize;
        let mut removed = false;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            if let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") {
                let mut remove_at = None;
                for (i, child) in info.children.iter().enumerate() {
                    let Node::Tag(entry) = child else { continue };
                    if entry.name != "None" {
                        continue;
                    }
                    if count == index {
                        remove_at = Some(i);
                        break;
                    }
                    count += 1;
                }
                if let Some(i) = remove_at {
                    info.children.remove(i);
                    removed = true;
                }
            }
            // Drop polygons for this GUID, and all owned fields when the complex main is gone.
            if let Some(polygons) = find_tag_mut(&mut pipette.children, "Polygons") {
                polygons.children.retain(|n| {
                    let Node::Tag(entry) = n else {
                        return true;
                    };
                    if entry.name != "None" {
                        return true;
                    }
                    let g = find_attrib(&entry.children, "GUID").map(|a| u32_le(&a.content));
                    let oid = find_attrib(&entry.children, "ComplexOwnerID").and_then(|a| {
                        if a.content.len() >= 8 {
                            Some(u64::from_le_bytes(a.content[0..8].try_into().ok()?))
                        } else if a.content.len() >= 4 {
                            Some(u32_le(&a.content) as u64)
                        } else {
                            None
                        }
                    });
                    if g == Some(guid) {
                        return false;
                    }
                    if drop_owned_polygons {
                        if let (Some(o), Some(own)) = (oid, owner) {
                            if o == own {
                                return false;
                            }
                        }
                    }
                    true
                });
            }
        }
        if !removed {
            return Err(StampError::MissingField);
        }
        self.buildings.remove(index);
        recompute_bounds(&mut self.document, &self.buildings);
        self.refresh_buildings();
        Ok(())
    }

    pub fn set_building_owner(&mut self, index: usize, owner: u64) -> Result<(), StampError> {
        let mut count = 0usize;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                if count == index {
                    if let Some(a) = find_attrib_mut(&mut entry.children, "ComplexOwnerID") {
                        let mut bytes = vec![0u8; a.content.len().max(8)];
                        bytes[0..8].copy_from_slice(&owner.to_le_bytes());
                        a.content = bytes;
                    } else {
                        entry.children.push(Node::Attrib(Attrib {
                            id: 0,
                            name: "ComplexOwnerID".into(),
                            content: owner.to_le_bytes().to_vec(),
                        }));
                    }
                    if let Some(b) = self.buildings.get_mut(index) {
                        b.complex_owner_id = Some(owner);
                    }
                    return Ok(());
                }
                count += 1;
            }
        }
        Err(StampError::MissingField)
    }

    /// Clone the first building entry as template and place `guid` at (x,y).
    /// If the stamp is empty, uses an embedded seed building entry.
    /// `variation`: pipette Variation (e.g. warehouse marsh CFG = `Some(1)`).
    pub fn add_building_from_template(
        &mut self,
        guid: u32,
        x: f32,
        y: f32,
        direction: i32,
        variation: Option<u32>,
    ) -> Result<(), StampError> {
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            let template = info.children.iter().find_map(|n| match n {
                Node::Tag(t) if t.name == "None" => Some(t.clone()),
                _ => None,
            });
            let mut entry = match template {
                Some(t) => t,
                None => seed_building_template_tag()?,
            };
            let template_guid = find_attrib(&entry.children, "GUID")
                .map(|a| u32_le(&a.content))
                .unwrap_or(guid);
            // Always write Pos — warehouse templates omit it (game treats as 0,0),
            // but aqueduct cisterns and every other placed building need an explicit Pos.
            if let Some(pos) = find_attrib_mut(&mut entry.children, "Pos") {
                let mut bytes = vec![0u8; 8];
                bytes[0..4].copy_from_slice(&x.to_le_bytes());
                bytes[4..8].copy_from_slice(&y.to_le_bytes());
                pos.content = bytes;
            } else {
                let mut bytes = vec![0u8; 8];
                bytes[0..4].copy_from_slice(&x.to_le_bytes());
                bytes[4..8].copy_from_slice(&y.to_le_bytes());
                entry.children.push(Node::Attrib(Attrib {
                    id: 0,
                    name: "Pos".into(),
                    content: bytes,
                }));
            }
            if let Some(g) = find_attrib_mut(&mut entry.children, "GUID") {
                let mut content = g.content.clone();
                if content.len() < 4 {
                    content.resize(4, 0);
                }
                content[0..4].copy_from_slice(&guid.to_le_bytes());
                g.content = content;
            }
            let _ = write_direction(&mut entry, direction);
            // Marsh / land CFG selector (Warehouse Celtic: 0=6×6, 1=5×5).
            if let Some(v) = variation.filter(|v| *v > 0) {
                if let Some(a) = find_attrib_mut(&mut entry.children, "Variation") {
                    let mut bytes = vec![0u8; a.content.len().max(4)];
                    bytes[0..4].copy_from_slice(&v.to_le_bytes());
                    a.content = bytes;
                } else {
                    entry.children.push(Node::Attrib(Attrib {
                        id: 0,
                        name: "Variation".into(),
                        content: v.to_le_bytes().to_vec(),
                    }));
                }
            } else {
                entry
                    .children
                    .retain(|n| !matches!(n, Node::Attrib(a) if a.name == "Variation"));
            }
            info.children.push(Node::Tag(entry));
            self.buildings.push(Building {
                guid,
                x,
                y,
                direction,
                complex_owner_id: None,
                variation,
            });
            clone_polygon_for_new_building(&mut self.document, template_guid, guid, x, y);
            recompute_bounds(&mut self.document, &self.buildings);
            self.refresh_buildings();
            return Ok(());
        }
        Err(StampError::MissingField)
    }

    pub fn set_building_guid(&mut self, index: usize, guid: u32) -> Result<(), StampError> {
        let mut count = 0usize;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                if count == index {
                    let Some(g) = find_attrib_mut(&mut entry.children, "GUID") else {
                        return Err(StampError::MissingField);
                    };
                    let mut content = g.content.clone();
                    if content.len() < 4 {
                        content.resize(4, 0);
                    }
                    content[0..4].copy_from_slice(&guid.to_le_bytes());
                    g.content = content;
                    if let Some(b) = self.buildings.get_mut(index) {
                        b.guid = guid;
                    }
                    return Ok(());
                }
                count += 1;
            }
        }
        Err(StampError::MissingField)
    }

    /// Set pipette `Variation` (e.g. warehouse Land=`None` / Marsh=`Some(1)`).
    pub fn set_building_variation(
        &mut self,
        index: usize,
        variation: Option<u32>,
    ) -> Result<(), StampError> {
        let mut count = 0usize;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                if count == index {
                    if let Some(v) = variation.filter(|v| *v > 0) {
                        if let Some(a) = find_attrib_mut(&mut entry.children, "Variation") {
                            let mut bytes = vec![0u8; a.content.len().max(4)];
                            bytes[0..4].copy_from_slice(&v.to_le_bytes());
                            a.content = bytes;
                        } else {
                            entry.children.push(Node::Attrib(Attrib {
                                id: 0,
                                name: "Variation".into(),
                                content: v.to_le_bytes().to_vec(),
                            }));
                        }
                    } else {
                        entry
                            .children
                            .retain(|n| !matches!(n, Node::Attrib(a) if a.name == "Variation"));
                    }
                    if let Some(b) = self.buildings.get_mut(index) {
                        b.variation = variation.filter(|v| *v > 0);
                    }
                    return Ok(());
                }
                count += 1;
            }
        }
        Err(StampError::MissingField)
    }
}

#[cfg(test)]
mod cistern_roundtrip {
    use super::*;
    use crate::catalog::{blocker_min_for, size_for_guid};
    use crate::coverage::footprint_cells;

    /// Editor TL flush to road at x=0 ⇒ written Pos must be the decentered pivot,
    /// not the geometric center. Neighbor buildings stay centered.
    ///
    /// Numbers (road column x=0, cistern TL x=1, size 3×3):
    /// - OLD centered write: Pos=(2.5, 1.5) → game TL=2 → 1 grass gap
    /// - NEW decentered write: Pos=(1.5, 1.5) → game TL=1 → flush
    #[test]
    fn cistern_flush_to_road_encodes_decentered_pos_not_center() {
        let road_x = 0i32;
        let tl = (road_x + 1, 0i32); // flush
        let (w, h) = size_for_guid(19753, 3.0, 3.0, None, None);
        assert!((w - 3.0).abs() < 1e-5 && (h - 3.0).abs() < 1e-5);

        let (mx, my) = blocker_min_for(19753, w, h, None, None);
        let cistern_pos = (tl.0 as f32 - mx, tl.1 as f32 - my);
        let neighbor_pos = (tl.0 as f32 + 1.5, tl.1 as f32 + 4.5); // centered 3×3 below

        assert!(
            (cistern_pos.0 - 1.5).abs() < 1e-5,
            "cistern Pos.x must be 1.5 (pivot), not centered 2.5; got {}",
            cistern_pos.0
        );
        assert!(
            (neighbor_pos.0 - 2.5).abs() < 1e-5,
            "neighbor stays centered at 2.5"
        );

        let mut stamp = Stamp::blank().expect("blank");
        stamp
            .add_building_from_template(19753, cistern_pos.0, cistern_pos.1, 0, None)
            .expect("cistern");
        stamp
            .add_building_from_template(2878, neighbor_pos.0, neighbor_pos.1, 0, None)
            .expect("neighbor");

        let encoded = encode_pipette_b64(&stamp).expect("encode");
        let back = decode_pipette_b64(&encoded).expect("decode");

        let mut raw_pos: Vec<(u32, f32, f32)> = Vec::new();
        for root in &back.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag(&pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                let guid = find_attrib(&entry.children, "GUID")
                    .map(|a| u32_le(&a.content))
                    .unwrap_or(0);
                let pos = find_attrib(&entry.children, "Pos")
                    .and_then(|a| f32x2(&a.content))
                    .unwrap_or((0.0, 0.0));
                raw_pos.push((guid, pos.0, pos.1));
            }
        }
        assert_eq!(raw_pos.len(), 2, "raw BuildingInfo entries: {raw_pos:?}");
        let cistern = raw_pos.iter().find(|p| p.0 == 19753).expect("cistern raw");
        let neighbor = raw_pos.iter().find(|p| p.0 == 2878).expect("neighbor raw");
        assert!(
            (cistern.1 - 1.5).abs() < 1e-5 && (cistern.2 - 1.5).abs() < 1e-5,
            "encoded cistern Pos must be (1.5,1.5), got ({},{})",
            cistern.1,
            cistern.2
        );
        assert!(
            (neighbor.1 - 2.5).abs() < 1e-5,
            "encoded neighbor Pos.x must stay 2.5, got {}",
            neighbor.1
        );

        let b = back
            .buildings
            .iter()
            .find(|b| b.guid == 19753)
            .expect("cistern building");
        let min_x = footprint_cells(b, None).iter().map(|c| c.x).min().unwrap();
        assert_eq!(min_x, 1, "editor footprint TL.x must be 1 (flush to road@0)");
    }

    #[test]
    fn cistern_pos_survives_encode_decode() {
        let mut stamp = Stamp::blank().expect("blank");
        stamp
            .add_building_from_template(19753, 1.5, 1.5, 0, None)
            .expect("add");
        let encoded = encode_pipette_b64(&stamp).expect("encode");
        let reloaded = decode_pipette_b64(&encoded).expect("decode");
        let b = reloaded
            .buildings
            .iter()
            .find(|b| b.guid == 19753)
            .expect("cistern");
        assert!((b.x - 1.5).abs() < 1e-5 && (b.y - 1.5).abs() < 1e-5);
        assert_eq!(footprint_cells(b, None).iter().map(|c| c.x).min(), Some(1));
    }
}


