#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::{bail, Context, Result};
use eframe::egui::{self as egui, ColorImage, TextureFilter, TextureHandle, Vec2, Key, Modifiers};
use eframe::{self};
use rfd::FileDialog;
use std::path::{Path, PathBuf};

mod cti;
use cti::{CTIDecoder, CTIHeader, CompressionId};

fn main() -> Result<()> {
    let native_options = eframe::NativeOptions::default();
    // Nepropagujeme eframe::Error přes `?` (není Send/Sync); mapneme na anyhow::Error (string).
    eframe::run_native(
        "CTI View",
        native_options,
        Box::new(|_cc| Ok(Box::<App>::default())),
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(())
}

#[derive(Default)]
struct App {
    image_tex: Option<TextureHandle>,
    image_size: Option<(u32, u32)>,
    last_path: Option<PathBuf>,

    // zoom & režimy zobrazení
    zoom: f32,            // 1.0 = 100%
    fit_to_window: bool,  // true = obsah se přizpůsobí oknu

    // info dialog
    show_info: bool,
    last_hdr: Option<CTIHeader>,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Top toolbar
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open…").clicked() {
                    let dir = self
                        .last_path
                        .as_deref()
                        .and_then(Path::parent)
                        .unwrap_or_else(|| Path::new("."));
                    let file = FileDialog::new()
                        .add_filter("CTI images", &["cti"])
                        .set_directory(dir)
                        .pick_file();

                    if let Some(path) = file {
                        if let Err(e) = self.load_cti(ctx, &path) {
                            eprintln!("open error: {e:?}");
                        } else {
                            self.last_path = Some(path);
                        }
                    }
                }

                if ui
                    .add_enabled(self.image_tex.is_some(), egui::Button::new("Info"))
                    .clicked()
                {
                    self.show_info = true;
                }

                ui.separator();

                // Fit to window
                if ui
                    .add_enabled(self.image_tex.is_some(), egui::Button::new("Fit"))
                    .clicked()
                {
                    self.fit_to_window = true;
                }

                // 1:1 (100 %)
                if ui
                    .add_enabled(self.image_tex.is_some(), egui::Button::new("1:1"))
                    .clicked()
                {
                    self.fit_to_window = false;
                    self.zoom = 1.0;
                }

                ui.separator();

                // Zoom - / +
                if ui
                    .add_enabled(self.image_tex.is_some(), egui::Button::new("Zoom -"))
                    .clicked()
                {
                    self.fit_to_window = false;
                    self.zoom = (self.zoom * 0.9).max(0.05);
                }
                if ui
                    .add_enabled(self.image_tex.is_some(), egui::Button::new("Zoom +"))
                    .clicked()
                {
                    self.fit_to_window = false;
                    self.zoom = (self.zoom * 1.1).min(50.0);
                }
                ui.label(format!("Zoom: {:.1}×", self.zoom));
            });
        });

        // Klávesová zkratka Cmd/Ctrl+0 → 1:1
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Num0)) {
            self.fit_to_window = false;
            self.zoom = 1.0;
        }

        // Scroll zoom (egui 0.32 API): posun kolečka mění zoom; také vypne Fit
        if ctx.input(|i| i.raw_scroll_delta.y != 0.0) && self.image_tex.is_some() {
            let delta = ctx.input(|i| i.raw_scroll_delta.y);
            let factor = if delta > 0.0 { 1.1 } else { 0.9 };
            self.fit_to_window = false;
            self.zoom = (self.zoom * factor).clamp(0.05, 50.0);
        }

        // Střední panel s obrázkem
        egui::CentralPanel::default().show(ctx, |ui| {
            if let (Some(tex), Some((w, h))) = (&self.image_tex, self.image_size) {
                let desired_size = if self.fit_to_window {
                    // Přizpůsobit oknu: neukládej scale do self.zoom, ať 1:1 zůstane přesné při přepnutí
                    let avail = ui.available_size();
                    let scale = (avail.x / w as f32).min(avail.y / h as f32);
                    Vec2::new(w as f32 * scale, h as f32 * scale)
                } else {
                    Vec2::new(w as f32 * self.zoom, h as f32 * self.zoom)
                };

                // Nové API: Image::new(&TextureHandle).fit_to_exact_size(size)
                ui.add(egui::Image::new(tex).fit_to_exact_size(desired_size));
            } else {
                ui.centered_and_justified(|ui| ui.label("Open a .cti file"));
            }
        });

        // Info okno
        if self.show_info {
            egui::Window::new("CTI Info")
                .collapsible(false)
                .resizable(true)
                .open(&mut self.show_info)
                .show(ctx, |ui| {
                    if let Some(h) = self.last_hdr {
                        ui.monospace(format!("Version    : {}", h.version));
                        ui.monospace(format!("Size       : {} x {}", h.width, h.height));
                        ui.monospace(format!(
                            "Tiles      : {} x {}  (tile={})",
                            h.tiles_x, h.tiles_y, h.tile_size
                        ));
                        ui.monospace(format!(
                            "ColorType  : {} ({})",
                            h.color_type,
                            color_name(h.color_type)
                        ));
                        let comp = CompressionId::from(h.compression);
                        ui.monospace(format!(
                            "Compression: {} ({})",
                            h.compression,
                            comp.describe()
                        ));
                        ui.monospace(format!("Quality    : {}", h.quality));
                        ui.monospace(format!(
                            "Flags      : 0x{:04X}  (RCT:{})",
                            h.flags,
                            (h.flags & 1) != 0
                        ));
                    } else {
                        ui.label("No file loaded.");
                    }
                });
        }
    }
}

impl App {
    fn load_cti(&mut self, ctx: &egui::Context, path: &PathBuf) -> Result<()> {
        // Načíst hlavičku pro Info
        let hdr_only = CTIDecoder::info(path)?;
        let (hdr, raw) =
            CTIDecoder::decode_file(path).with_context(|| format!("decode {:?}", path))?;
        debug_assert_eq!(hdr_only.width, hdr.width);
        self.last_hdr = Some(hdr);

        let image = match hdr.color_type {
            1 => {
                // L8 → RGBA8
                let mut rgba = Vec::with_capacity((hdr.width * hdr.height * 4) as usize);
                for &l in &raw {
                    rgba.extend_from_slice(&[l, l, l, 255]);
                }
                ColorImage::from_rgba_unmultiplied(
                    [hdr.width as usize, hdr.height as usize],
                    &rgba,
                )
            }
            3 => {
                // RGB8 → RGBA8
                let mut rgba = Vec::with_capacity((hdr.width * hdr.height * 4) as usize);
                for px in raw.chunks_exact(3) {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                ColorImage::from_rgba_unmultiplied(
                    [hdr.width as usize, hdr.height as usize],
                    &rgba,
                )
            }
            4 => {
                // RGBA8 (přímo)
                ColorImage::from_rgba_unmultiplied(
                    [hdr.width as usize, hdr.height as usize],
                    &raw,
                )
            }
            2 | 5 => {
                bail!("16-bit preview not implemented yet (L16/RGB16).");
            }
            _ => bail!("Unsupported ColorType ID {}", hdr.color_type),
        };

        let tex = ctx.load_texture(
            "cti-image",
            image,
            egui::TextureOptions {
                magnification: TextureFilter::Linear,
                minification: TextureFilter::Linear,
                ..Default::default()
            },
        );
        self.image_tex = Some(tex);
        self.image_size = Some((hdr.width, hdr.height));
        self.zoom = 1.0;
        self.fit_to_window = true; // po otevření defaultně vyplň okno
        Ok(())
    }
}

fn color_name(id: u8) -> &'static str {
    match id {
        1 => "L8",
        2 => "L16",
        3 => "RGB8",
        4 => "RGBA8",
        5 => "RGB16",
        _ => "Unknown",
    }
}
