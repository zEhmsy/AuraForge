//! Renders `assets/logo.svg` into the executables' icons: resource 1 is the
//! logo (Explorer, taskbar, running tray icon), resource 2 a dimmed copy for
//! the tray while the daemon is stopped.

use std::path::Path;
use std::{env, fs};

use resvg::{tiny_skia, usvg};

const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

fn main() {
    println!("cargo:rerun-if-changed=assets/logo.svg");
    println!("cargo:rerun-if-changed=build.rs");
    let out = env::var("OUT_DIR").unwrap();
    let out = Path::new(&out);

    let svg = fs::read("assets/logo.svg").expect("assets/logo.svg");
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default()).expect("valid logo.svg");
    let images: Vec<tiny_skia::Pixmap> = SIZES.iter().map(|&size| render(&tree, size)).collect();

    fs::write(out.join("logo.ico"), ico(&images)).unwrap();
    let dimmed: Vec<_> = images.into_iter().map(dim).collect();
    fs::write(out.join("logo-off.ico"), ico(&dimmed)).unwrap();

    let ico_path = |name: &str| out.join(name).display().to_string().replace('\\', "/");
    let rc = format!(
        "1 ICON \"{}\"\n2 ICON \"{}\"\n",
        ico_path("logo.ico"),
        ico_path("logo-off.ico")
    );
    let rc_path = out.join("app.rc");
    fs::write(&rc_path, rc).unwrap();
    embed_resource::compile(&rc_path, embed_resource::NONE).manifest_required().unwrap();
}

fn render(tree: &usvg::Tree, size: u32) -> tiny_skia::Pixmap {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).unwrap();
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap
}

/// Grayscale at reduced brightness, so "stopped" reads at a glance.
fn dim(mut pixmap: tiny_skia::Pixmap) -> tiny_skia::Pixmap {
    for px in pixmap.data_mut().chunks_exact_mut(4) {
        let luma = (0.3 * f32::from(px[0]) + 0.59 * f32::from(px[1]) + 0.11 * f32::from(px[2])) * 0.6;
        px[..3].fill(luma as u8);
    }
    pixmap
}

/// ICO file with one PNG-compressed entry per image.
fn ico(images: &[tiny_skia::Pixmap]) -> Vec<u8> {
    let pngs: Vec<Vec<u8>> = images.iter().map(|p| p.encode_png().unwrap()).collect();
    let mut file = Vec::new();
    file.extend([0, 0, 1, 0]);
    file.extend((pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (image, png) in images.iter().zip(&pngs) {
        // 256 is stored as 0.
        let side = image.width() as u8;
        file.extend([side, side, 0, 0]);
        file.extend(1u16.to_le_bytes());
        file.extend(32u16.to_le_bytes());
        file.extend((png.len() as u32).to_le_bytes());
        file.extend(offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in pngs {
        file.extend(png);
    }
    file
}
