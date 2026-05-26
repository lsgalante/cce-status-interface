use std::fs::File;

fn main() {
    let file = File::open("/home/lsgalante/.gemini/antigravity/brain/2aeef5fd-2989-49f9-a513-b732668c813b/current_screen_unified.png").unwrap();
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let width = info.width as usize;
    let height = info.height as usize;
    println!("Width: {}, Height: {}", width, height);

    println!("Top row y=4 colors (x = 0, 100, 500, 1000, 2000, 3000, 3800):");
    for x in &[0, 100, 500, 1000, 2000, 3000, 3800] {
        let idx = (4 * width + x) * 4;
        println!("x={}: R={}, G={}, B={}, A={}", x, buf[idx], buf[idx+1], buf[idx+2], buf[idx+3]);
    }

    println!("Row y=20 colors (x = 0, 100, 500, 1000, 2000, 3000, 3800):");
    for x in &[0, 100, 500, 1000, 2000, 3000, 3800] {
        let idx = (20 * width + x) * 4;
        println!("x={}: R={}, G={}, B={}, A={}", x, buf[idx], buf[idx+1], buf[idx+2], buf[idx+3]);
    }
}
