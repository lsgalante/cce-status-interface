use std::fs::File;

fn main() {
    let file = File::open("/home/lsgalante/.gemini/antigravity/brain/2aeef5fd-2989-49f9-a513-b732668c813b/current_screen_3.png").unwrap();
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    
    println!("Truly bright pixels for x = 3310..3390, y = 10..50:");
    let mut found = 0;
    for y in 10..50 {
        for x in 3310..3390 {
            let idx = (y * info.width as usize + x) * 4;
            let r = buf[idx];
            let g = buf[idx + 1];
            let b = buf[idx + 2];
            let a = buf[idx + 3];
            
            if r > 100 && g > 100 && b > 100 {
                found += 1;
                println!("y={}, x={}: R={}, G={}, B={}, A={}", y, x, r, g, b, a);
            }
        }
    }
    println!("Total truly bright pixels: {}", found);
}
