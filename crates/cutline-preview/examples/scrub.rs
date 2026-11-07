//! `cargo run --release -p cutline-preview --example scrub -- file.mp4 [fps]`
//! Simulates timeline scrubbing with the preview player and reports seek
//! latency and how many distinct frames reach the screen. Use it to check
//! mpv option changes in `src/mpv.rs`.
use cutline_preview::Player;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// About the viewer's size in a default window.
const W: usize = 1130;
const H: usize = 636;

/// A cheap fingerprint, to count frames that actually look different.
fn fingerprint(rgba: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    rgba.iter().step_by(4099).for_each(|b| b.hash(&mut h));
    h.finish()
}

/// Pump the player until it shows a frame within `tol` of `target`.
fn wait_at(p: &mut Player, target: f64, tol: f64) -> Option<Duration> {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(5) {
        if p.update(W, H).is_some() && (p.position() - target).abs() <= tol {
            return Some(t0.elapsed());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    None
}

/// Drag from `a` to `b` over 2 s, seeking every 8 ms like pointer motion.
/// Returns distinct frames shown and the time to the exact frame at `b`.
fn drag(p: &mut Player, a: f64, b: f64, tol: f64) -> (usize, Option<Duration>) {
    let dur = Duration::from_secs(2);
    let t0 = Instant::now();
    let mut seen = HashSet::new();
    let mut last_send = t0 - Duration::from_secs(1);
    while t0.elapsed() < dur {
        if last_send.elapsed() >= Duration::from_millis(8) {
            let k = t0.elapsed().as_secs_f64() / dur.as_secs_f64();
            p.seek(a + (b - a) * k.min(1.0));
            last_send = Instant::now();
        }
        if let Some(f) = p.update(W, H) {
            seen.insert(fingerprint(&f.rgba));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    p.seek(b);
    (seen.len(), wait_at(p, b, tol))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(file) = args.get(1).map(std::path::Path::new) else {
        eprintln!("usage: scrub FILE [FPS]");
        std::process::exit(2);
    };
    let fps: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(60.0);
    // Recordings often have uneven timestamps; allow a couple of frames.
    let tol = 2.5 / fps;
    let mut p = Player::new(std::path::Path::new("ffmpeg"), Arc::new(|| {}));
    let t0 = Instant::now();
    p.open(file, 0.0, false);
    wait_at(&mut p, 0.0, 1.0);
    println!("open: {} ms", t0.elapsed().as_millis());

    // Targets spread through keyframe intervals, near and far from a keyframe.
    let mut lat: Vec<u128> = (0..12)
        .filter_map(|i| {
            let t = 1.0 + i as f64 * 1.37;
            p.seek(t);
            wait_at(&mut p, t, tol).map(|d| d.as_millis())
        })
        .collect();
    lat.sort();
    if let (Some(mid), Some(max)) = (lat.get(lat.len() / 2), lat.last()) {
        println!(
            "exact seek: median {mid} ms, max {max} ms ({} of 12 landed)",
            lat.len()
        );
    }

    for (name, a, b) in [
        ("fast drag 1 s -> 15 s", 1.0, 15.0),
        ("slow drag 4.0 s -> 4.5 s", 4.0, 4.5),
    ] {
        let (frames, settle) = drag(&mut p, a, b, tol);
        println!(
            "{name}: {frames} distinct frames in 2 s, exact frame {} ms after release",
            settle.map_or("never".into(), |d| d.as_millis().to_string())
        );
    }

    // Playback must still keep up.
    p.seek(1.0);
    wait_at(&mut p, 1.0, tol);
    p.set_paused(false);
    let t0 = Instant::now();
    let mut seen = HashSet::new();
    while t0.elapsed() < Duration::from_secs(3) {
        if let Some(f) = p.update(W, H) {
            seen.insert(fingerprint(&f.rgba));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    p.set_paused(true);
    println!(
        "playback: {:.0} distinct frames/s, played {:.2} s of video in 3 s",
        seen.len() as f64 / 3.0,
        p.position() - 1.0
    );
}
