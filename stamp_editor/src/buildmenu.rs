//! Ingame-like construction menu hierarchy (from assets.xml extract).

use crate::catalog::RegionFilter;
use crate::library::EditorConfig;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct ConstructionMenuFile {
    pub regions: HashMap<String, RegionMenu>,
    #[serde(default)]
    pub building_icons: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegionMenu {
    pub tabs: Vec<MenuTab>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MenuTab {
    pub id: String,
    pub guid: u32,
    pub label: String,
    pub role: String,
    #[serde(default)]
    pub category_icon: Option<String>,
    #[serde(default)]
    pub secondary_icon: Option<String>,
    pub entries: Vec<MenuEntry>,
}

impl MenuTab {
    pub fn is_need(&self) -> bool {
        self.role == "need"
    }

    pub fn is_infrastructure(&self) -> bool {
        self.role == "infrastructure"
    }

    /// Rough Roman numeral for tier tabs (fallback if secondary_icon missing).
    pub fn tier_mark(&self) -> &'static str {
        let l = self.label.to_lowercase();
        if l.contains("t1") || l.contains("waders") || l.contains("peasants") || l.contains("liberti")
        {
            "I"
        } else if l.contains("t2")
            || l.contains("smith")
            || l.contains("mercator")
            || l.contains("plebe")
        {
            "II"
        } else if l.contains("t3")
            || l.contains("alderm")
            || l.contains("noble")
            || l.contains("equit")
        {
            "III"
        } else if l.contains("t4") || l.contains("patric") {
            "IV"
        } else {
            "·"
        }
    }

    pub fn short_tier_label(&self) -> String {
        let l = self.label.trim();
        for prefix in ["T1 ", "T2 ", "T3 ", "T4 "] {
            if let Some(rest) = l.strip_prefix(prefix) {
                return rest.to_string();
            }
        }
        l.to_string()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind")]
pub enum MenuEntry {
    #[serde(rename = "building")]
    Building {
        guid: u32,
        name: String,
        #[serde(default)]
        short_name: Option<String>,
        #[serde(default)]
        icon: Option<String>,
    },
    #[serde(rename = "chain")]
    Chain {
        guid: u32,
        name: String,
        #[serde(default)]
        short_name: Option<String>,
        #[serde(default)]
        icon: Option<String>,
        #[serde(default)]
        buildings: Vec<u32>,
    },
    #[serde(rename = "category")]
    Category {
        guid: u32,
        name: String,
        #[serde(default)]
        short_name: Option<String>,
        #[serde(default)]
        icon: Option<String>,
        #[serde(default)]
        entries: Vec<MenuEntry>,
    },
}

impl MenuEntry {
    pub fn label(&self) -> String {
        match self {
            MenuEntry::Building { short_name, name, .. }
            | MenuEntry::Chain { short_name, name, .. }
            | MenuEntry::Category { short_name, name, .. } => short_name
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| cleanup_name(name)),
        }
    }

    pub fn guid(&self) -> u32 {
        match self {
            MenuEntry::Building { guid, .. }
            | MenuEntry::Chain { guid, .. }
            | MenuEntry::Category { guid, .. } => *guid,
        }
    }

    pub fn icon(&self) -> Option<&str> {
        match self {
            MenuEntry::Building { icon, .. }
            | MenuEntry::Chain { icon, .. }
            | MenuEntry::Category { icon, .. } => icon.as_deref(),
        }
    }

    /// Hide unresolved DLC stubs / empty GUID buttons in the UI.
    pub fn is_displayable(&self) -> bool {
        match self {
            MenuEntry::Building { name, icon, .. } => {
                !name.starts_with("GUID ") && icon.as_ref().is_some_and(|s| !s.is_empty())
            }
            MenuEntry::Chain { buildings, .. } => !buildings.is_empty(),
            MenuEntry::Category { entries, icon, .. } => {
                entries.iter().any(|e| e.is_displayable())
                    || icon.as_ref().is_some_and(|s| !s.is_empty())
            }
        }
    }
}

fn cleanup_name(name: &str) -> String {
    name.replace("&amp;", "&")
        .replace("Production Coast ", "")
        .replace("Production Marsh ", "")
        .replace("Production Field ", "")
        .replace("Production ", "")
        .replace("Public ", "")
        .replace("Street ", "")
        .replace("Warehouse ", "")
        .replace("Residence ", "")
}

#[derive(Debug, Clone, Default)]
pub struct ConstructionMenu {
    pub regions: HashMap<String, RegionMenu>,
    pub building_icons: HashMap<u32, String>,
}

impl ConstructionMenu {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let file: ConstructionMenuFile =
            serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let building_icons = file
            .building_icons
            .into_iter()
            .filter_map(|(k, v)| k.parse::<u32>().ok().map(|g| (g, v)))
            .collect();
        Ok(Self {
            regions: file.regions,
            building_icons,
        })
    }

    pub fn load_default() -> Self {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("_gamedata").join("construction_menu.json"));
                candidates.push(dir.join("construction_menu.json"));
                candidates.push(
                    dir.join("../../..")
                        .join("_gamedata")
                        .join("construction_menu.json"),
                );
            }
        }
        candidates.push(
            EditorConfig::config_dir()
                .join("_gamedata")
                .join("construction_menu.json"),
        );
        if let Ok(cwd) = env::current_dir() {
            candidates.push(cwd.join("_gamedata").join("construction_menu.json"));
            candidates.push(cwd.join("..").join("_gamedata").join("construction_menu.json"));
            candidates.push(
                cwd.join("../..")
                    .join("_gamedata")
                    .join("construction_menu.json"),
            );
        }
        candidates
            .into_iter()
            .find(|p| p.is_file())
            .and_then(|p| Self::load(&p).ok())
            .unwrap_or_default()
    }

    pub fn region_key(filter: RegionFilter) -> Option<&'static str> {
        match filter {
            RegionFilter::Latium => Some("Roman"),
            RegionFilter::Albion => Some("Celtic"),
            RegionFilter::Delta => Some("Egyptian"),
            RegionFilter::All => None,
        }
    }

    pub fn tabs_for(&self, filter: RegionFilter) -> &[MenuTab] {
        let Some(key) = Self::region_key(filter) else {
            return &[];
        };
        self.regions
            .get(key)
            .map(|r| r.tabs.as_slice())
            .unwrap_or(&[])
    }

    pub fn infrastructure_tab(&self, filter: RegionFilter) -> Option<&MenuTab> {
        self.tabs_for(filter).iter().find(|t| t.is_infrastructure())
    }

    pub fn need_tabs(&self, filter: RegionFilter) -> Vec<(usize, &MenuTab)> {
        self.tabs_for(filter)
            .iter()
            .enumerate()
            .filter(|(_, t)| t.is_need())
            .collect()
    }

    pub fn building_icon(&self, guid: u32) -> Option<&str> {
        self.building_icons.get(&guid).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_celtic_tabs() {
        let menu = ConstructionMenu::load_default();
        let tabs = menu.tabs_for(RegionFilter::Albion);
        assert!(tabs.len() >= 5, "expected celtic tabs, got {}", tabs.len());
        assert!(tabs.iter().any(|t| t.label.contains("Waders") || t.label.contains("T1")));
        assert!(tabs.iter().any(|t| t.role == "materials"));
        let waders = tabs.iter().find(|t| t.label.contains("Waders")).expect("waders");
        assert!(
            waders.category_icon.as_deref().unwrap_or("").contains("portrait"),
            "need tabs carry construction-menu portraits"
        );
    }

    #[test]
    fn drinking_horns_chain_has_buildings() {
        let menu = ConstructionMenu::load_default();
        let tabs = menu.tabs_for(RegionFilter::Albion);
        let mut found = None;
        for tab in tabs {
            for e in &tab.entries {
                if let MenuEntry::Chain {
                    short_name,
                    buildings,
                    ..
                } = e
                {
                    if short_name
                        .as_deref()
                        .unwrap_or("")
                        .contains("Drinking")
                        || e.label().contains("Drinking")
                    {
                        found = Some(buildings.clone());
                    }
                }
            }
        }
        if found.is_none() {
            for tab in tabs {
                for e in &tab.entries {
                    if let MenuEntry::Chain { buildings, .. } = e {
                        if buildings.contains(&55963) {
                            found = Some(buildings.clone());
                        }
                    }
                }
            }
        }
        let blds = found.expect("Drinking Horns chain in celtic menu");
        assert!(blds.contains(&55963), "main building in chain: {blds:?}");
        assert!(blds.len() >= 2, "chain should list intermediates: {blds:?}");
    }

    #[test]
    fn entries_have_icons() {
        let menu = ConstructionMenu::load_default();
        let tabs = menu.tabs_for(RegionFilter::Albion);
        let need = tabs.iter().find(|t| t.is_need()).unwrap();
        let with_icon = need.entries.iter().filter(|e| e.icon().is_some()).count();
        assert!(with_icon >= 3, "need entries should carry goods icons");
    }

    #[test]
    fn egyptian_delta_menu_loads() {
        let menu = ConstructionMenu::load_default();
        assert!(ConstructionMenu::region_key(RegionFilter::All).is_none());
        let tabs = menu.tabs_for(RegionFilter::Delta);
        assert!(
            tabs.iter().any(|t| t.label.contains("Thresher") || t.label.contains("T1")),
            "egyptian tiers: {:?}",
            tabs.iter().map(|t| &t.label).collect::<Vec<_>>()
        );
        assert!(tabs.iter().all(|t| t.entries.iter().all(|e| e.is_displayable())));
    }
}

