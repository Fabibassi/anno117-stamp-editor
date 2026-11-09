mod app;

use app::StampEditorApp;
use eframe::egui;
use std::env;
use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let catalog = PathBuf::from(
        r"C:\Users\fabi_\Projects\anno117-stamp-editor\_gamedata\editor_catalog.json",
    );
    let mut open_path: Option<PathBuf> = None;
    let mut open_stamp: Option<String> = None;
    let mut args = env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--open" => open_path = args.next().map(PathBuf::from),
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

