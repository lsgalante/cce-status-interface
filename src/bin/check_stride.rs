use std::fs::File;

fn main() {
    let file = File::open("/home/lsgalante/.gemini/antigravity/brain/2aeef5fd-2989-49f9-a513-b732668c813b/current_screen_unified_2.png").unwrap();
    let decoder = png::Decoder::new(file);
    let reader = decoder.read_info().unwrap();
    let info = reader.info();
    println!("PNG Info: width={}, height={}, color_type={:?}, bit_depth={:?}, raw_bytes={}", 
        info.width, info.height, info.color_type, info.bit_depth, info.raw_bytes());
}
