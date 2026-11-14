//! Regenerate assets/icon.png from the procedural fallback icon.
//!
//! Run with: cargo run --release --example gen_icon

fn main() {
    let bytes = karaoke_kit::icon::png_bytes(512).expect("icon generation failed");
    let path = std::path::Path::new("assets/icon.png");
    std::fs::create_dir_all("assets").expect("assets directory");
    std::fs::write(path, bytes).expect("write icon");
    println!("Wrote {}", path.display());
}
