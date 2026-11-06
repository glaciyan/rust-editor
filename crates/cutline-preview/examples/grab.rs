//! `cargo run -p cutline-preview --example grab -- file.mp4 2.5 out.ppm`
//! Seeks with the player and writes the frame it shows. A smoke test for libmpv.
use std::sync::Arc;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut p =
        cutline_preview::Player::new(std::path::Path::new("/usr/bin/ffmpeg"), Arc::new(|| {}));
    println!("sound: {}", p.has_sound());
    p.open(std::path::Path::new(&a[1]), 0.0, false);
    let t: f64 = a[2].parse().unwrap();
    let start = std::time::Instant::now();
    let mut seeked = false;
    let mut last = None;
    loop {
        if let Some(f) = p.update(640, 360) {
            last = Some(f);
        }
        let f = if start.elapsed().as_millis() > 2000 {
            last.take()
        } else {
            None
        };
        if !seeked && start.elapsed().as_millis() > 300 {
            p.seek(t);
            seeked = true;
        }
        if let Some(f) = f {
            if seeked {
                let mut out = format!("P6\n{} {}\n255\n", f.width, f.height).into_bytes();
                for px in f.rgba.chunks(4) {
                    out.extend_from_slice(&px[..3]);
                }
                std::fs::write(&a[3], out).unwrap();
                println!("pos {:.3} after {:?}", p.position(), start.elapsed());
                break;
            }
        }
        if start.elapsed().as_secs() > 10 {
            println!("timeout pos {}", p.position());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}
