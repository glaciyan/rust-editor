use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The ffmpeg/ffprobe pair Cutline drives.
#[derive(Clone, Debug)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub caps: Capabilities,
}

#[derive(Clone, Debug, Default)]
pub struct Capabilities {
    pub version: String,
    /// Major version, when it parses (`git` builds report `None` and are treated as new).
    pub major: Option<u32>,
    pub libx265: bool,
    pub libx264: bool,
    pub libopus: bool,
    pub aac: bool,
    /// The `noise` bitstream filter with `drop` expressions (ffmpeg 6+),
    /// needed for frame-exact cuts without re-encoding everything.
    pub smart_cut: bool,
}

fn candidates(name: &str) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(dir) = std::env::var("CUTLINE_FFMPEG_DIR") {
        out.push(Path::new(&dir).join(name));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join(name));
            out.push(dir.join("ffmpeg").join(name));
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for p in std::env::split_paths(&paths) {
            out.push(p.join(name));
        }
    }
    out
}

fn find(name: &str) -> Option<PathBuf> {
    candidates(name).into_iter().find(|p| p.is_file())
}

impl Tools {
    /// Find ffmpeg and ffprobe and check what they can do.
    pub fn locate() -> Result<Tools, String> {
        let ffmpeg = find("ffmpeg").ok_or("Cutline needs ffmpeg. Install it with your package manager (for example `sudo apt install ffmpeg`).")?;
        let ffprobe = ffmpeg
            .parent()
            .map(|d| d.join("ffprobe"))
            .filter(|p| p.is_file())
            .or_else(|| find("ffprobe"))
            .ok_or("Cutline needs ffprobe, which comes with ffmpeg.")?;
        let caps = Capabilities::detect(&ffmpeg);
        Ok(Tools {
            ffmpeg,
            ffprobe,
            caps,
        })
    }
}

fn output_of(bin: &Path, args: &[&str]) -> String {
    Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

impl Capabilities {
    pub fn detect(ffmpeg: &Path) -> Capabilities {
        let version_out = output_of(ffmpeg, &["-hide_banner", "-version"]);
        let version = version_out.lines().next().unwrap_or("").to_string();
        let major = parse_major(&version);
        let enc = output_of(ffmpeg, &["-hide_banner", "-encoders"]);
        let has = |n: &str| enc.lines().any(|l| l.split_whitespace().nth(1) == Some(n));
        let bsfs = output_of(ffmpeg, &["-hide_banner", "-bsfs"]);
        let noise = bsfs.lines().any(|l| l.trim() == "noise");
        Capabilities {
            version,
            major,
            libx265: has("libx265"),
            libx264: has("libx264"),
            libopus: has("libopus"),
            aac: has("aac"),
            smart_cut: noise && major.map(|m| m >= 6).unwrap_or(true),
        }
    }
}

fn parse_major(line: &str) -> Option<u32> {
    let v = line
        .strip_prefix("ffmpeg version ")?
        .split_whitespace()
        .next()?;
    let v = v.trim_start_matches('n');
    let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions() {
        assert_eq!(
            super::parse_major("ffmpeg version 6.1.1-3ubuntu5 Copyright"),
            Some(6)
        );
        assert_eq!(
            super::parse_major("ffmpeg version n7.0.2 Copyright"),
            Some(7)
        );
        assert_eq!(
            super::parse_major("ffmpeg version N-112233-gabc Copyright"),
            None
        );
    }
}
