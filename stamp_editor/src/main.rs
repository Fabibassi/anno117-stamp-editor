// Kein schwarzes CMD-Fenster neben der GUI (auch Debug).
#![windows_subsystem = "windows"]

mod app;

use app::StampEditorApp;
use eframe::egui;
use stamp_editor::library::EditorConfig;
use stamp_editor::pack::fix_cistern_pivots_path;
use std::env;
use std::path::PathBuf;
use std::process;

/// Resolve `editor_catalog.json` for release EXE + local dev without hardcoding one PC.
fn resolve_catalog_path() -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("_gamedata").join("editor_catalog.json"));
            candidates.push(dir.join("editor_catalog.json"));
        }
    }
    candidates.push(
        EditorConfig::config_dir()
            .join("_gamedata")
            .join("editor_catalog.json"),
    );
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join("_gamedata").join("editor_catalog.json"));
        candidates.push(cwd.join("..").join("_gamedata").join("editor_catalog.json"));
    }
    // Dev machine fallback (optional).
    candidates.push(PathBuf::from(
        r"C:\Users\fabi_\Projects\anno117-stamp-editor\_gamedata\editor_catalog.json",
    ));
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .unwrap_or_else(|| {
            EditorConfig::config_dir()
                .join("_gamedata")
                .join("editor_catalog.json")
        })
}

fn main() -> eframe::Result<()> {
    let catalog = resolve_catalog_path();
    let mut open_path: Option<PathBuf> = None;
    let mut open_stamp: Option<String> = None;
    let mut fix_cisterns: Option<PathBuf> = None;
    let mut args = env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--open" => open_path = args.next().map(PathBuf::from),
            "--fix-cisterns" => {
                fix_cisterns = args
                    .next()
                    .map(PathBuf::from)
                    .or_else(|| open_path.clone())
                    .or_else(|| env::current_dir().ok());
            }
            "--stamp" => {
                // Join tokens until the next flag so "Olive Oil x3 v2" survives
                // shells that split on spaces without proper quoting.
                let mut parts = Vec::new();
                while let Some(next) = args.peek() {
                    if next.starts_with('-') {
                        break;
                    }
                    parts.push(args.next().unwrap());
                }
                if !parts.is_empty() {
                    open_stamp = Some(parts.join(" "));
                }
            }
            other if other.ends_with(".json") => open_path = Some(PathBuf::from(other)),
            _ => {}
        }
    }

    if let Some(root) = fix_cisterns {
        let report = fix_cistern_pivots_path(&root);
        println!(
            "Cistern pivot fix: packs={} stamps={} rewritten={} buildings_fixed={}",
            report.packs_scanned,
            report.stamps_scanned,
            report.stamps_rewritten,
            report.buildings_fixed
        );
        for err in &report.errors {
            eprintln!("  warn: {err}");
        }
        process::exit(if report.errors.is_empty() { 0 } else { 1 });
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_title("Anno 117 Stamp Editor"),
        ..Default::default()
    };
    eframe::run_native(
        "Anno 117 Stamp Editor",
        options,
        Box::new(move |_cc| {
            let mut app = StampEditorApp::new(catalog);
            if let Some(path) = open_path {
                if let Some(name) = open_stamp {
                    app.open_named(path, &name);
                } else {
                    app.open_path_public(path);
                }
            }
            Ok(Box::new(app))
        }),
    )
}
