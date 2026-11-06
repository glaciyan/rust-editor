use cutline_core::{Profile, Segment};
use cutline_ffmpeg::MediaInfo;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Everything the export dialog decided.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportRequest {
    pub segments: Vec<Segment>,
    pub profile: Profile,
    pub output_dir: PathBuf,
    /// File name without extension, e.g. `holiday-clip (cut)`.
    pub name: String,
    /// One file per part instead of one joined video.
    pub separate: bool,
    /// Audio tracks to keep, by position in the first video's track list.
    pub audio_tracks: Vec<usize>,
    /// The user agreed to replace existing files.
    pub overwrite: bool,
}

impl ExportRequest {
    pub fn total_duration(&self) -> f64 {
        self.segments.iter().map(|s| s.len().secs()).sum()
    }

    pub fn extension(&self, first: Option<&MediaInfo>) -> String {
        let src_ext = first.map(|m| m.extension()).unwrap_or_else(|| "mp4".into());
        self.profile.container.extension(&src_ext)
    }

    /// Where the finished files go, in order.
    pub fn output_paths(&self, ext: &str) -> Vec<PathBuf> {
        output_paths(
            &self.output_dir,
            &self.name,
            ext,
            self.separate,
            self.segments.len(),
        )
    }
}

pub fn output_paths(
    dir: &Path,
    name: &str,
    ext: &str,
    separate: bool,
    parts: usize,
) -> Vec<PathBuf> {
    if !separate || parts <= 1 {
        return vec![dir.join(format!("{name}.{ext}"))];
    }
    let base = name.strip_suffix(" (cut)").unwrap_or(name);
    (1..=parts)
        .map(|n| dir.join(format!("{base} (part {n}).{ext}")))
        .collect()
}

/// The default name: `<stem> (cut)`.
pub fn default_name(source: &Path) -> String {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "video".into());
    format!("{stem} (cut)")
}

/// A name that doesn't clash with existing files: `x (cut)`, `x (cut 2)`, …
pub fn free_name(dir: &Path, name: &str, ext: &str, separate: bool, parts: usize) -> String {
    let taken = |n: &str| {
        output_paths(dir, n, ext, separate, parts)
            .iter()
            .any(|p| p.exists())
    };
    if !taken(name) {
        return name.to_string();
    }
    let (base, inner) = match name.strip_suffix(')').and_then(|n| n.rsplit_once(" (")) {
        Some((b, inner)) => (b.to_string(), Some(inner.to_string())),
        None => (name.to_string(), None),
    };
    for i in 2..1000 {
        let candidate = match &inner {
            Some(inner) => {
                let word = inner.trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ');
                format!("{base} ({word} {i})")
            }
            None => format!("{base} ({i})"),
        };
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{name} {}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(
            default_name(Path::new("/v/holiday-clip.mp4")),
            "holiday-clip (cut)"
        );
        let p = output_paths(Path::new("/v"), "a (cut)", "mp4", true, 2);
        assert_eq!(p[1], Path::new("/v/a (part 2).mp4"));
        let dir = std::env::temp_dir().join(format!("cutline-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a (cut).mp4"), b"x").unwrap();
        assert_eq!(free_name(&dir, "a (cut)", "mp4", false, 1), "a (cut 2)");
        std::fs::write(dir.join("a (cut 2).mp4"), b"x").unwrap();
        assert_eq!(free_name(&dir, "a (cut)", "mp4", false, 1), "a (cut 3)");
        assert_eq!(free_name(&dir, "b", "mp4", false, 1), "b");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
