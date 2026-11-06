use std::ffi::OsString;
use std::path::PathBuf;

/// One input with its own options (`-ss 3 -t 5 -i file`).
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub args: Vec<String>,
    pub path: PathBuf,
}

impl Input {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Input {
            args: vec![],
            path: path.into(),
        }
    }
    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }
    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, a: I) -> Self {
        self.args.extend(a.into_iter().map(Into::into));
        self
    }
}

/// A typed ffmpeg invocation. Built by the planner, turned into argv by
/// [`FfmpegCommand::argv`] and shown verbatim in a job's Details.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FfmpegCommand {
    pub inputs: Vec<Input>,
    pub filter_complex: Option<String>,
    pub output_args: Vec<String>,
    /// `-` means "no file" (used with `-f null` for first passes).
    pub output: PathBuf,
}

impl FfmpegCommand {
    pub fn new(output: impl Into<PathBuf>) -> Self {
        FfmpegCommand {
            output: output.into(),
            ..Default::default()
        }
    }
    pub fn input(mut self, i: Input) -> Self {
        self.inputs.push(i);
        self
    }
    pub fn filter(mut self, f: impl Into<String>) -> Self {
        self.filter_complex = Some(f.into());
        self
    }
    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.output_args.push(a.into());
        self
    }
    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, a: I) -> Self {
        self.output_args.extend(a.into_iter().map(Into::into));
        self
    }

    /// Fixed leading options: quiet banner, never read stdin, overwrite our
    /// own temp files, machine-readable progress on stdout.
    pub const PREAMBLE: [&'static str; 8] = [
        "-hide_banner",
        "-nostdin",
        "-y",
        "-v",
        "warning",
        "-progress",
        "pipe:1",
        "-nostats",
    ];

    pub fn argv(&self) -> Vec<OsString> {
        let mut v: Vec<OsString> = Self::PREAMBLE.iter().map(OsString::from).collect();
        for i in &self.inputs {
            v.extend(i.args.iter().map(OsString::from));
            v.push("-i".into());
            v.push(i.path.clone().into_os_string());
        }
        if let Some(f) = &self.filter_complex {
            v.push("-filter_complex".into());
            v.push(f.into());
        }
        v.extend(self.output_args.iter().map(OsString::from));
        v.push(self.output.clone().into_os_string());
        v
    }

    /// A copy-pasteable shell line.
    pub fn display(&self, program: &str) -> String {
        let mut parts = vec![shell_quote(program)];
        parts.extend(
            self.argv()
                .iter()
                .map(|a| shell_quote(&a.to_string_lossy())),
        );
        parts.join(" ")
    }
}

pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=+,%@".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn argv_order() {
        let c = FfmpegCommand::new("out.mp4")
            .input(Input::new("in file.mp4").args(["-ss", "1.000000"]))
            .args(["-c", "copy"]);
        let s = c.display("ffmpeg");
        assert!(
            s.ends_with("-ss 1.000000 -i 'in file.mp4' -c copy out.mp4"),
            "{s}"
        );
    }
}
