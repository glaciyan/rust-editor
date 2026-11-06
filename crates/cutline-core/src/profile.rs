//! Export profiles. The normal UI only ever shows a profile's name and
//! description; the fields below are for the Advanced editor.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    /// Use the first source's container (mp4 stays mp4, mkv stays mkv).
    Source,
    Mp4,
    Mkv,
    Mov,
    /// Audio-only MPEG-4.
    M4a,
}

impl Container {
    pub fn extension(self, source_ext: &str) -> String {
        match self {
            Container::Source => match source_ext.to_ascii_lowercase().as_str() {
                e @ ("mp4" | "mkv" | "mov" | "m4v" | "webm") => e.to_string(),
                _ => "mp4".to_string(),
            },
            Container::Mp4 => "mp4".into(),
            Container::Mkv => "mkv".into(),
            Container::Mov => "mov".into(),
            Container::M4a => "m4a".into(),
        }
    }
    pub const ALL: [Container; 5] = [
        Container::Source,
        Container::Mp4,
        Container::Mkv,
        Container::Mov,
        Container::M4a,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Container::Source => "Same as source",
            Container::Mp4 => "MP4",
            Container::Mkv => "Matroska (MKV)",
            Container::Mov => "QuickTime (MOV)",
            Container::M4a => "M4A (audio only)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    /// Copy the video stream; only the frames around each cut are re-encoded.
    Copy,
    H265,
    H264,
    /// Drop video.
    None,
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 4] = [
        VideoCodec::Copy,
        VideoCodec::H265,
        VideoCodec::H264,
        VideoCodec::None,
    ];
    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::Copy => "Copy (smart cut)",
            VideoCodec::H265 => "H.265 / HEVC (libx265)",
            VideoCodec::H264 => "H.264 / AVC (libx264)",
            VideoCodec::None => "No video",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "mode", content = "value")]
pub enum RateControl {
    /// Constant rate factor; lower is better quality.
    Crf(u8),
    /// Average bitrate in kbit/s, two-pass.
    Bitrate(u32),
    /// Hit a file size in megabytes (10^6 bytes); solved per export.
    TargetSize(f64),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoSettings {
    pub codec: VideoCodec,
    pub rate: RateControl,
    /// x264/x265 preset name (`ultrafast` … `veryslow`).
    pub preset: String,
    /// Scale down so the height is at most this. `None` keeps the source size.
    pub max_height: Option<u32>,
    /// Cap the frame rate. `None` keeps the source rate.
    pub max_fps: Option<f64>,
    /// Force a keyframe at least every N frames.
    #[serde(default)]
    pub keyframe_interval: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    Copy,
    Aac,
    Opus,
    None,
}

impl AudioCodec {
    pub const ALL: [AudioCodec; 4] = [
        AudioCodec::Copy,
        AudioCodec::Aac,
        AudioCodec::Opus,
        AudioCodec::None,
    ];
    pub fn label(self) -> &'static str {
        match self {
            AudioCodec::Copy => "Copy",
            AudioCodec::Aac => "AAC",
            AudioCodec::Opus => "Opus",
            AudioCodec::None => "No audio",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioSettings {
    pub codec: AudioCodec,
    /// Per track, kbit/s. Ignored for Copy.
    pub bitrate_kbps: u32,
    /// Downmix to at most this many channels.
    #[serde(default)]
    pub max_channels: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// One plain sentence shown under the name in the export dialog.
    pub description: String,
    pub container: Container,
    pub video: VideoSettings,
    pub audio: AudioSettings,
    #[serde(skip)]
    pub builtin: bool,
}

pub const ORIGINAL_ID: &str = "original";
pub const FIT_ID: &str = "fit";

impl Profile {
    pub fn is_fit(&self) -> bool {
        matches!(self.video.rate, RateControl::TargetSize(_))
            && self.video.codec != VideoCodec::Copy
    }
    pub fn is_copy(&self) -> bool {
        self.video.codec == VideoCodec::Copy
    }
    pub fn audio_only(&self) -> bool {
        self.video.codec == VideoCodec::None
    }

    /// Original quality: copy everything, smart-cut at the edges.
    pub fn original() -> Profile {
        Profile {
            id: ORIGINAL_ID.into(),
            name: "Original quality".into(),
            description: "Same quality as your video. Fastest.".into(),
            container: Container::Source,
            video: VideoSettings {
                codec: VideoCodec::Copy,
                rate: RateControl::Crf(16),
                preset: "medium".into(),
                max_height: None,
                max_fps: None,
                keyframe_interval: None,
            },
            audio: AudioSettings {
                codec: AudioCodec::Copy,
                bitrate_kbps: 192,
                max_channels: None,
            },
            builtin: true,
        }
    }

    /// Fit to size: H.265 two-pass at a solved bitrate.
    pub fn fit(target_mb: f64) -> Profile {
        Profile {
            id: FIT_ID.into(),
            name: "Fit to size".into(),
            description: "Pick a size; Cutline finds the best quality that fits.".into(),
            container: Container::Mp4,
            video: VideoSettings {
                codec: VideoCodec::H265,
                rate: RateControl::TargetSize(target_mb),
                preset: "medium".into(),
                max_height: None,
                max_fps: None,
                keyframe_interval: None,
            },
            audio: AudioSettings {
                codec: AudioCodec::Aac,
                bitrate_kbps: 128,
                max_channels: Some(2),
            },
            builtin: true,
        }
    }

    /// Named presets shown in the Presets card.
    pub fn presets() -> Vec<Profile> {
        vec![
            Profile {
                id: "smaller".into(),
                name: "Smaller file".into(),
                description: "Much smaller, still looks good.".into(),
                container: Container::Mp4,
                video: VideoSettings {
                    codec: VideoCodec::H265,
                    rate: RateControl::Crf(27),
                    preset: "medium".into(),
                    max_height: None,
                    max_fps: None,
                    keyframe_interval: None,
                },
                audio: AudioSettings {
                    codec: AudioCodec::Aac,
                    bitrate_kbps: 128,
                    max_channels: Some(2),
                },
                builtin: true,
            },
            Profile {
                id: "share".into(),
                name: "Plays anywhere".into(),
                description: "Opens on any phone, browser or TV.".into(),
                container: Container::Mp4,
                video: VideoSettings {
                    codec: VideoCodec::H264,
                    rate: RateControl::Crf(21),
                    preset: "medium".into(),
                    max_height: Some(1080),
                    max_fps: Some(60.0),
                    keyframe_interval: None,
                },
                audio: AudioSettings {
                    codec: AudioCodec::Aac,
                    bitrate_kbps: 160,
                    max_channels: Some(2),
                },
                builtin: true,
            },
            Profile {
                id: "editing".into(),
                name: "For editing".into(),
                description: "Big file that editing apps scrub smoothly.".into(),
                container: Container::Mov,
                video: VideoSettings {
                    codec: VideoCodec::H264,
                    rate: RateControl::Crf(14),
                    preset: "fast".into(),
                    max_height: None,
                    max_fps: None,
                    keyframe_interval: Some(12),
                },
                audio: AudioSettings {
                    codec: AudioCodec::Aac,
                    bitrate_kbps: 256,
                    max_channels: None,
                },
                builtin: true,
            },
            Profile {
                id: "audio".into(),
                name: "Audio only".into(),
                description: "Just the sound, as a music file.".into(),
                container: Container::M4a,
                video: VideoSettings {
                    codec: VideoCodec::None,
                    rate: RateControl::Crf(23),
                    preset: "medium".into(),
                    max_height: None,
                    max_fps: None,
                    keyframe_interval: None,
                },
                audio: AudioSettings {
                    codec: AudioCodec::Aac,
                    bitrate_kbps: 192,
                    max_channels: None,
                },
                builtin: true,
            },
        ]
    }
}

/// What the export dialog says about quality. Never more specific than this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Quality {
    Low,
    Okay,
    Good,
    Great,
}

impl Quality {
    pub fn label(self) -> &'static str {
        match self {
            Quality::Great => "Great quality",
            Quality::Good => "Good quality",
            Quality::Okay => "Okay quality",
            Quality::Low => "Low quality",
        }
    }
    /// Filled bars out of four.
    pub fn bars(self) -> usize {
        match self {
            Quality::Great => 4,
            Quality::Good => 3,
            Quality::Okay => 2,
            Quality::Low => 1,
        }
    }
    /// For CRF-based presets: x264/x265 CRF to a plain label.
    pub fn from_crf(codec: VideoCodec, crf: u8) -> Quality {
        // x265 CRF ~ x264 CRF + 4 for similar looks.
        let c = if codec == VideoCodec::H265 {
            crf.saturating_sub(4)
        } else {
            crf
        };
        match c {
            0..=19 => Quality::Great,
            20..=24 => Quality::Good,
            25..=29 => Quality::Okay,
            _ => Quality::Low,
        }
    }
}
