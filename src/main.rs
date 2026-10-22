#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod app;
mod commands;
mod dialogs;
mod document;
mod editor;
mod encoding;
mod export;
mod find;
mod inspector;
mod panels;
mod search;
mod settings;
mod transform;
mod values;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::APP_NAME)
            .with_app_id("hexeditor")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0])
            .with_drag_and_drop(true)
            .with_icon(app_icon()),
        ..Default::default()
    };

    eframe::run_native(
        app::APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(app::HexApp::new(cc, files)))),
    )
}

/// Procedurally drawn window icon: a dark tile with a grid of colored "byte" cells.
fn app_icon() -> egui::IconData {
    const SIZE: usize = 64;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    let palette = [
        [90, 160, 255],
        [110, 210, 140],
        [235, 200, 110],
        [200, 150, 255],
        [255, 135, 100],
    ];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let i = (y * SIZE + x) * 4;
            // Rounded-square mask.
            let (dx, dy) = (
                (x as f32 - 31.5).abs() - 22.0,
                (y as f32 - 31.5).abs() - 22.0,
            );
            let outside = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - 8.0;
            if outside > 1.0 {
                continue;
            }
            let alpha = ((1.0 - outside).clamp(0.0, 1.0) * 255.0) as u8;
            let mut color = [32u8, 36, 46];
            // 4x4 cell grid inside the tile.
            let (gx, gy) = (x as i32 - 10, y as i32 - 10);
            if (0..44).contains(&gx) && (0..44).contains(&gy) && gx % 11 < 8 && gy % 11 < 8 {
                let cell = (gx / 11 + gy / 11 * 4) as usize;
                color = palette[(cell * 7 + cell / 3) % palette.len()];
            }
            rgba[i..i + 4].copy_from_slice(&[color[0], color[1], color[2], alpha]);
        }
    }
    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}
