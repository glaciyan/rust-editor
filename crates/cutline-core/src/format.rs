//! Plain-words formatting: sizes, durations, "ready in about 1 min".

/// `75.4` → `1:15`, `3725` → `1:02:05`.
pub fn clock(secs: f64) -> String {
    let s = if secs.is_finite() { secs.max(0.0) } else { 0.0 };
    let total = s.floor() as u64;
    let (h, m, sec) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// Clock with tenths, for handles: `1:15.4`.
pub fn clock_precise(secs: f64) -> String {
    let s = secs.max(0.0);
    let tenths = ((s * 10.0).floor() as u64) % 10;
    format!("{}.{}", clock(s), tenths)
}

/// Short length for labels: `16 s`, `2 min 5 s`, `1 h 3 min`.
pub fn length(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 10.0 {
        let v = (s * 10.0).round() / 10.0;
        if v.fract() == 0.0 {
            format!("{v:.0} s")
        } else {
            format!("{v:.1} s")
        }
    } else if s < 60.0 {
        format!("{:.0} s", s.round())
    } else if s < 3600.0 {
        let t = s.round() as u64;
        if t % 60 == 0 {
            format!("{} min", t / 60)
        } else {
            format!("{} min {} s", t / 60, t % 60)
        }
    } else {
        let t = (s / 60.0).round() as u64;
        if t % 60 == 0 {
            format!("{} h", t / 60)
        } else {
            format!("{} h {} min", t / 60, t % 60)
        }
    }
}

/// Bytes as a human size, decimal megabytes like file managers show: `19.4 MB`.
pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b < 1e3 {
        format!("{bytes} bytes")
    } else if b < 1e6 {
        format!("{:.0} KB", b / 1e3)
    } else if b < 1e8 {
        format!("{:.1} MB", b / 1e6)
    } else if b < 1e9 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.1} GB", b / 1e9)
    }
}

/// Rounded estimate for the export dialog: `about 61 MB`.
pub fn about_size(bytes: f64) -> String {
    let b = bytes.max(0.0);
    let mb = b / 1e6;
    if mb < 1.0 {
        "less than 1 MB".to_string()
    } else if mb < 10.0 {
        format!("about {:.0} MB", mb.round().max(1.0))
    } else if mb < 1000.0 {
        // two significant figures reads as an estimate, not a promise
        let mag = 10f64.powf((mb.log10()).floor() - 1.0);
        format!("about {:.0} MB", (mb / mag).round() * mag)
    } else {
        format!("about {:.1} GB", mb / 1000.0)
    }
}

/// `ready in seconds`, `ready in about 1 min`, `ready in about 25 min`.
pub fn ready_in(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 20.0 {
        "ready in seconds".into()
    } else if s < 45.0 {
        "ready in under a minute".into()
    } else if s < 90.0 {
        "ready in about 1 min".into()
    } else if s < 3600.0 * 1.5 {
        format!("ready in about {} min", (s / 60.0).round() as u64)
    } else {
        format!("ready in about {:.0} h", (s / 3600.0).round())
    }
}

/// `about 20 s left`, `about 3 min left`.
pub fn time_left(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 5.0 {
        "almost done".into()
    } else if s < 60.0 {
        format!("about {} s left", ((s / 5.0).round() * 5.0) as u64)
    } else if s < 3600.0 {
        format!("about {} min left", (s / 60.0).round() as u64)
    } else {
        format!("about {:.1} h left", s / 3600.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn words() {
        assert_eq!(clock(62.0), "1:02");
        assert_eq!(clock(760.0), "12:40");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(length(16.2), "16 s");
        assert_eq!(length(4.0), "4 s");
        assert_eq!(length(125.0), "2 min 5 s");
        assert_eq!(size(19_400_000), "19.4 MB");
        assert_eq!(about_size(61_300_000.0), "about 61 MB");
        assert_eq!(about_size(4_400_000.0), "about 4 MB");
        assert_eq!(about_size(234_000_000.0), "about 230 MB");
        assert_eq!(ready_in(3.0), "ready in seconds");
        assert_eq!(ready_in(70.0), "ready in about 1 min");
    }
}
