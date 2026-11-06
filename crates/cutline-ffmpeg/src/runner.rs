use crate::{FfmpegCommand, Tools};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Clone, Debug, Default)]
pub struct RunLog {
    pub command: String,
    pub stderr: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("canceled")]
    Canceled,
    #[error("ffmpeg stopped with an error")]
    Failed { code: Option<i32>, log: RunLog },
    #[error("couldn't start ffmpeg: {0}")]
    Spawn(std::io::Error),
}

/// Run one ffmpeg command. `out_duration` is how long the output will be
/// (seconds) and turns ffmpeg's `out_time` into a 0..1 fraction.
pub fn run(
    tools: &Tools,
    cmd: &FfmpegCommand,
    out_duration: f64,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f64),
) -> Result<RunLog, RunError> {
    let command = cmd.display(&tools.ffmpeg.to_string_lossy());
    let mut child = Command::new(&tools.ffmpeg)
        .args(cmd.argv())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(RunError::Spawn)?;

    let stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let (tx, rx) = std::sync::mpsc::channel::<f64>();
    let progress_thread = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(v) = line
                .strip_prefix("out_time_us=")
                .or_else(|| line.strip_prefix("out_time_ms="))
            {
                if let Ok(us) = v.trim().parse::<i64>() {
                    let _ = tx.send(us as f64 / 1e6);
                }
            }
        }
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        buf
    });

    let status = loop {
        while let Ok(t) = rx.try_recv() {
            if out_duration > 0.0 {
                on_progress((t / out_duration).clamp(0.0, 1.0));
            }
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = progress_thread.join();
            let _ = err_thread.join();
            return Err(RunError::Canceled);
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(RunError::Spawn(e)),
        }
    };
    let _ = progress_thread.join();
    let stderr = err_thread.join().unwrap_or_default();
    let log = RunLog { command, stderr };
    if status.success() {
        on_progress(1.0);
        Ok(log)
    } else {
        Err(RunError::Failed {
            code: status.code(),
            log,
        })
    }
}
