use crate::Tools;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoStream {
    /// Absolute stream index in the file.
    pub index: usize,
    pub codec: String,
    pub profile: Option<String>,
    pub width: u32,
    pub height: u32,
    pub pix_fmt: String,
    pub fps: f64,
    /// `r_frame_rate` as a rational, kept exact for re-encoding.
    pub fps_num: u32,
    pub fps_den: u32,
    pub bit_rate: Option<u64>,
    pub color_primaries: Option<String>,
    pub color_trc: Option<String>,
    pub color_space: Option<String>,
    pub color_range: Option<String>,
    /// Counter-clockwise degrees from the display matrix.
    pub rotation: i32,
    pub bit_depth: u32,
}

impl VideoStream {
    /// Size as shown to the viewer (after rotation).
    pub fn display_size(&self) -> (u32, u32) {
        if self.rotation.rem_euclid(180) == 90 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioStream {
    pub index: usize,
    pub codec: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub bit_rate: Option<u64>,
    pub language: Option<String>,
    pub title: Option<String>,
}

impl AudioStream {
    /// Plain name for the Audio checkboxes: "English", "Commentary", "Track 2".
    pub fn label(&self, position: usize) -> String {
        if let Some(t) = self.title.as_deref().filter(|t| !t.trim().is_empty()) {
            return t.trim().to_string();
        }
        if let Some(l) = self.language.as_deref() {
            if let Some(name) = language_name(l) {
                return name.to_string();
            }
        }
        format!("Track {}", position + 1)
    }
}

fn language_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "eng" | "en" => "English",
        "deu" | "ger" | "de" => "German",
        "fra" | "fre" | "fr" => "French",
        "spa" | "es" => "Spanish",
        "ita" | "it" => "Italian",
        "por" | "pt" => "Portuguese",
        "nld" | "dut" | "nl" => "Dutch",
        "jpn" | "ja" => "Japanese",
        "kor" | "ko" => "Korean",
        "zho" | "chi" | "zh" => "Chinese",
        "rus" | "ru" => "Russian",
        "pol" | "pl" => "Polish",
        "swe" | "sv" => "Swedish",
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub size_bytes: u64,
    /// Seconds.
    pub duration: f64,
    /// Container start time; ffmpeg `-ss` is relative to this, packet
    /// timestamps are not.
    pub start_time: f64,
    pub format_name: String,
    pub bit_rate: Option<u64>,
    pub video: Option<VideoStream>,
    pub audio: Vec<AudioStream>,
}

impl MediaInfo {
    pub fn extension(&self) -> String {
        self.path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    }
    /// Average bytes per second of the whole file, for size estimates.
    pub fn bytes_per_sec(&self) -> f64 {
        if self.duration > 0.0 {
            self.size_bytes as f64 / self.duration
        } else {
            0.0
        }
    }
    /// Video bitrate in bit/s: from the stream, else file minus audio.
    pub fn video_bitrate(&self) -> f64 {
        if let Some(b) = self.video.as_ref().and_then(|v| v.bit_rate) {
            return b as f64;
        }
        let audio: f64 = self
            .audio
            .iter()
            .map(|a| a.bit_rate.unwrap_or(128_000) as f64)
            .sum();
        (self.bytes_per_sec() * 8.0 - audio).max(0.0)
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(|x| x.as_str())
        .map(|x| x.to_string())
        .filter(|x| x != "unknown")
}
fn num<T: std::str::FromStr>(v: &Value, k: &str) -> Option<T> {
    match v.get(k)? {
        Value::String(x) => x.parse().ok(),
        Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}
fn ratio(r: &str) -> Option<(u32, u32)> {
    let (a, b) = r.split_once('/')?;
    let (a, b): (u32, u32) = (a.parse().ok()?, b.parse().ok()?);
    (a > 0 && b > 0).then_some((a, b))
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("Couldn't read {0}: {1}")]
    Io(String, std::io::Error),
    #[error("This doesn't look like a video Cutline can open.")]
    NotMedia(String),
}

pub fn probe(tools: &Tools, path: &Path) -> Result<MediaInfo, ProbeError> {
    let meta =
        std::fs::metadata(path).map_err(|e| ProbeError::Io(path.display().to_string(), e))?;
    let out = Command::new(&tools.ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| ProbeError::Io(path.display().to_string(), e))?;
    if !out.status.success() {
        return Err(ProbeError::NotMedia(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    let json: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| ProbeError::NotMedia(e.to_string()))?;
    parse_probe(&json, path, meta.len()).ok_or_else(|| ProbeError::NotMedia("no streams".into()))
}

pub fn parse_probe(json: &Value, path: &Path, size: u64) -> Option<MediaInfo> {
    let format = json.get("format")?;
    let streams = json.get("streams")?.as_array()?;
    let mut video = None;
    let mut audio = vec![];
    for st in streams {
        let index = num::<usize>(st, "index").unwrap_or(0);
        let disposition_pic = st
            .get("disposition")
            .and_then(|d| d.get("attached_pic"))
            .and_then(|x| x.as_i64())
            .unwrap_or(0)
            == 1;
        match st.get("codec_type").and_then(|x| x.as_str()) {
            Some("video") if video.is_none() && !disposition_pic => {
                let (fps_num, fps_den) = s(st, "r_frame_rate")
                    .and_then(|r| ratio(&r))
                    .filter(|(n, d)| (*n as f64 / *d as f64) <= 240.0)
                    .or_else(|| s(st, "avg_frame_rate").and_then(|r| ratio(&r)))
                    .unwrap_or((30, 1));
                let rotation = st
                    .get("side_data_list")
                    .and_then(|l| l.as_array())
                    .and_then(|l| l.iter().find_map(|sd| num::<f64>(sd, "rotation")))
                    .or_else(|| {
                        st.get("tags")
                            .and_then(|t| num::<f64>(t, "rotate"))
                            .map(|r| -r)
                    })
                    .unwrap_or(0.0)
                    .round() as i32;
                let pix_fmt = s(st, "pix_fmt").unwrap_or_else(|| "yuv420p".into());
                let bit_depth =
                    num::<u32>(st, "bits_per_raw_sample").unwrap_or(if pix_fmt.contains("10") {
                        10
                    } else if pix_fmt.contains("12") {
                        12
                    } else {
                        8
                    });
                video = Some(VideoStream {
                    index,
                    codec: s(st, "codec_name").unwrap_or_default(),
                    profile: s(st, "profile"),
                    width: num(st, "width").unwrap_or(0),
                    height: num(st, "height").unwrap_or(0),
                    pix_fmt,
                    fps: fps_num as f64 / fps_den as f64,
                    fps_num,
                    fps_den,
                    bit_rate: num(st, "bit_rate"),
                    color_primaries: s(st, "color_primaries"),
                    color_trc: s(st, "color_transfer"),
                    color_space: s(st, "color_space"),
                    color_range: s(st, "color_range"),
                    rotation,
                    bit_depth,
                });
            }
            Some("audio") => {
                let tags = st.get("tags");
                audio.push(AudioStream {
                    index,
                    codec: s(st, "codec_name").unwrap_or_default(),
                    channels: num(st, "channels").unwrap_or(2),
                    sample_rate: num(st, "sample_rate").unwrap_or(48000),
                    bit_rate: num(st, "bit_rate"),
                    language: tags.and_then(|t| s(t, "language")).filter(|l| l != "und"),
                    title: tags.and_then(|t| {
                        s(t, "title").or_else(|| s(t, "handler_name")).filter(|h| {
                            // mp4 handler names are usually boilerplate
                            !matches!(
                                h.as_str(),
                                "SoundHandler"
                                    | "Sound Media Handler"
                                    | "Core Media Audio"
                                    | "ISO Media file produced by Google Inc."
                            ) && !h.starts_with("Mainconcept")
                        })
                    }),
                });
            }
            _ => {}
        }
    }
    if video.is_none() && audio.is_empty() {
        return None;
    }
    Some(MediaInfo {
        path: path.to_path_buf(),
        size_bytes: size,
        duration: num(format, "duration").unwrap_or(0.0),
        start_time: num(format, "start_time").unwrap_or(0.0),
        format_name: s(format, "format_name").unwrap_or_default(),
        bit_rate: num(format, "bit_rate"),
        video,
        audio,
    })
}

/// One video packet, in decode order. Times are relative to the file start
/// (same clock as `-ss`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Packet {
    pub pts: f64,
    pub key: bool,
}

/// Video packets between `from` and `to` (seconds, `-ss` clock). ffprobe
/// starts reading at the keyframe at or before `from`.
pub fn packets(
    tools: &Tools,
    info: &MediaInfo,
    from: f64,
    to: f64,
) -> std::io::Result<Vec<Packet>> {
    let Some(v) = &info.video else {
        return Ok(vec![]);
    };
    let abs_from = (from + info.start_time).max(0.0);
    let abs_to = to + info.start_time;
    let out = Command::new(&tools.ffprobe)
        .args(["-v", "error", "-select_streams"])
        .arg(format!("{}", v.index))
        .args([
            "-show_entries",
            "packet=pts_time,flags",
            "-of",
            "csv=p=0",
            "-read_intervals",
        ])
        .arg(format!("{abs_from:.6}%{abs_to:.6}"))
        .arg(&info.path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    Ok(parse_packets(
        &String::from_utf8_lossy(&out.stdout),
        info.start_time,
    ))
}

pub fn parse_packets(csv: &str, start_time: f64) -> Vec<Packet> {
    csv.lines()
        .filter_map(|l| {
            let mut it = l.split(',');
            let pts: f64 = it.next()?.trim().parse().ok()?;
            let flags = it.next().unwrap_or("");
            Some(Packet {
                pts: pts - start_time,
                key: flags.starts_with('K'),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_probe_json() {
        let j: Value = serde_json::from_str(
            r#"{"streams":[{"index":0,"codec_type":"video","codec_name":"hevc","profile":"Main 10","width":3840,"height":2160,
            "pix_fmt":"yuv420p10le","r_frame_rate":"30000/1001","avg_frame_rate":"30000/1001","bit_rate":"40000000",
            "side_data_list":[{"side_data_type":"Display Matrix","rotation":-90}]},
            {"index":1,"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000","tags":{"language":"eng","handler_name":"SoundHandler"}}],
            "format":{"duration":"12.5","start_time":"0.000000","format_name":"mov,mp4,m4a,3gp,3g2,mj2","bit_rate":"41000000"}}"#,
        )
        .unwrap();
        let m = parse_probe(&j, Path::new("/x/clip.mp4"), 1000).unwrap();
        let v = m.video.unwrap();
        assert_eq!(v.codec, "hevc");
        assert_eq!(v.rotation, -90);
        assert_eq!(v.display_size(), (2160, 3840));
        assert_eq!(v.bit_depth, 10);
        assert!((v.fps - 29.97).abs() < 0.01);
        assert_eq!(m.audio[0].label(0), "English");
        assert_eq!(m.audio[0].title, None);
    }
    #[test]
    fn parses_packets() {
        let p = parse_packets("0.100000,K__\n0.133333,___\nN/A,___\n", 0.1);
        assert_eq!(p.len(), 2);
        assert!(p[0].key && p[0].pts.abs() < 1e-9);
    }
}
