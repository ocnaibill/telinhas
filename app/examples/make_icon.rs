//! Builds `assets/telinhas.ico` from the logo, with the sizes Windows asks
//! for. Run it again whenever the logo changes:
//!
//! ```text
//! cargo run -p telinhas --example make_icon
//! ```

use std::fs;

const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() {
    let images: Vec<(u32, Vec<u8>)> = SIZES
        .iter()
        .map(|&size| (size, telinhas::icon_png(size).expect("the logo renders")))
        .collect();

    // ICONDIR, one ICONDIRENTRY per image, then the PNG data. Windows has
    // read PNG inside .ico since Vista.
    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
    ico.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in &images {
        // A size of 256 is written as 0.
        let byte = if *size >= 256 { 0 } else { *size as u8 };
        ico.extend_from_slice(&[byte, byte, 0, 0]); // width, height, palette, reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // color planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &images {
        ico.extend_from_slice(png);
    }

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/telinhas.ico");
    fs::write(path, &ico).expect("writes the icon");
    println!("wrote {path} ({} bytes, {} sizes)", ico.len(), images.len());
}
