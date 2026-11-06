//! Running one export: gather facts, plan, run the steps, verify, retry, and
//! move the finished file into place. Every intermediate file lives in a
//! hidden scratch folder that is removed on success, failure and cancel, and
//! the final file only appears (by an atomic rename) once it is complete.

use crate::fit;
use crate::plan::{CutStyle, ExportPlan, Facts, OutputPlan, PlanError, Planner, Step};
use crate::request::ExportRequest;
use cutline_ffmpeg::{probe, runner, FfmpegCommand, Input, RunError, Tools};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub type JobId = u64;

/// What the Details view shows.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct JobDetails {
    pub commands: Vec<String>,
    pub log: String,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobOutput {
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub fraction: f64,
    pub eta_secs: Option<f64>,
    pub stage: String,
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("Canceled")]
    Canceled,
    #[error("{0}")]
    Plan(#[from] PlanError),
    #[error("{0}")]
    Failed(String),
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Turn an ffmpeg failure into one plain sentence.
fn explain(log: &str) -> String {
    let l = log.to_ascii_lowercase();
    if l.contains("no space left") {
        "The disk is full. Free some space or save somewhere else.".into()
    } else if l.contains("permission denied") || l.contains("read-only file system") {
        "Cutline isn't allowed to save there. Choose another folder.".into()
    } else if l.contains("no such file") {
        "One of the videos was moved or deleted.".into()
    } else {
        "Something went wrong while saving. Details has the full log.".into()
    }
}

pub struct Job<'a> {
    pub id: JobId,
    pub tools: &'a Tools,
    pub req: &'a ExportRequest,
    pub cancel: &'a AtomicBool,
}

struct Meter<'a> {
    total: f64,
    done: f64,
    started: Instant,
    on: &'a mut dyn FnMut(Progress),
}

impl Meter<'_> {
    fn report(&mut self, step_weight: f64, frac: f64, stage: &str) {
        let total = self.total.max(1e-9);
        let f = ((self.done + step_weight * frac) / total).clamp(0.0, 0.999);
        let elapsed = self.started.elapsed().as_secs_f64();
        let eta = (f > 0.03 && elapsed > 2.0).then(|| elapsed / f * (1.0 - f));
        (self.on)(Progress { fraction: f, eta_secs: eta, stage: stage.to_string() });
    }
}

impl<'a> Job<'a> {
    fn canceled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn facts(&self) -> Result<Facts, JobError> {
        let mut sources = HashMap::new();
        for s in &self.req.segments {
            if !sources.contains_key(&s.path) {
                let info = probe::probe(self.tools, &s.path).map_err(|e| JobError::Failed(e.to_string()))?;
                sources.insert(s.path.clone(), info);
            }
        }
        let mut packets = vec![];
        if self.req.profile.is_copy() {
            for s in &self.req.segments {
                let info = &sources[&s.path];
                // Read a little past the end so a keyframe-cut fallback finds the next keyframe.
                let p = probe::packets(self.tools, info, s.start.secs(), s.end.secs() + 12.0).map_err(|e| JobError::Failed(e.to_string()))?;
                packets.push(p);
                if self.canceled() {
                    return Err(JobError::Canceled);
                }
            }
        }
        Ok(Facts { sources, packets })
    }

    pub fn run(&self, on_progress: &mut dyn FnMut(Progress), details: &mut JobDetails) -> Result<Vec<JobOutput>, JobError> {
        let req = self.req;
        on_progress(Progress { fraction: 0.0, eta_secs: None, stage: "Getting ready".into() });
        let facts = self.facts()?;
        std::fs::create_dir_all(&req.output_dir)
            .map_err(|_| JobError::Failed("Cutline can't create that folder. Choose another one in Save to.".into()))?;
        let tmp = req.output_dir.join(format!(".cutline-{}-{}", std::process::id(), self.id));
        std::fs::create_dir_all(&tmp).map_err(|_| JobError::Failed("Cutline isn't allowed to save there. Choose another folder.".into()))?;
        let _scratch = Scratch(tmp.clone());
        let pass_dir = std::env::temp_dir().join(format!("cutline-pass-{}-{}", std::process::id(), self.id));
        std::fs::create_dir_all(&pass_dir).map_err(|e| JobError::Failed(e.to_string()))?;
        let _pass_scratch = Scratch(pass_dir.clone());
        let planner = Planner { req, facts: &facts, caps: &self.tools.caps, tmp: &tmp, pass_log_dir: &pass_dir };
        let plan: ExportPlan = planner.plan()?;
        details.notes.extend(plan.notes.iter().cloned());

        // Refuse to overwrite anything the user didn't agree to replace.
        if !req.overwrite {
            if let Some(p) = plan.outputs.iter().find(|o| o.final_path.exists()) {
                return Err(JobError::Failed(format!("{} already exists.", p.final_path.file_name().unwrap_or_default().to_string_lossy())));
            }
        }

        let total: f64 = plan.outputs.iter().flat_map(|o| o.steps.iter()).map(step_weight).sum();
        let mut meter = Meter { total, done: 0.0, started: Instant::now(), on: on_progress };
        let mut finished = vec![];
        for (oi, out) in plan.outputs.iter().enumerate() {
            let stage = if plan.outputs.len() > 1 { format!("Saving part {} of {}", oi + 1, plan.outputs.len()) } else { "Saving".into() };
            let ok = self.run_steps(&out.steps, &mut meter, &stage, details)?;
            if !ok && out.smart {
                // A junction didn't decode cleanly: redo this output with keyframe cuts.
                details.log.push_str("\n== Smart cut check failed; falling back to keyframe cuts ==\n");
                let fallback = planner.plan_with(CutStyle::Keyframe, None)?;
                let fb = &fallback.outputs[oi];
                details.notes.extend(fallback.notes.iter().cloned());
                if !self.run_steps(&fb.steps, &mut meter, &stage, details)? {
                    return Err(JobError::Failed("The saved video didn't play back cleanly. Details has the full log.".into()));
                }
            } else if !ok {
                return Err(JobError::Failed("The saved video didn't play back cleanly. Details has the full log.".into()));
            }
            if out.fit.is_some() {
                self.enforce_size(&planner, oi, out, &mut meter, &stage, details)?;
            }
            finished.push(out);
        }

        // Everything is complete: move into place.
        let mut outputs = vec![];
        for out in finished {
            if !req.overwrite && out.final_path.exists() {
                return Err(JobError::Failed(format!("{} already exists.", out.final_path.display())));
            }
            std::fs::rename(&out.temp_path, &out.final_path).map_err(|e| JobError::Failed(format!("Couldn't save the file: {e}")))?;
            let bytes = std::fs::metadata(&out.final_path).map(|m| m.len()).unwrap_or(0);
            outputs.push(JobOutput { path: out.final_path.clone(), bytes });
        }
        Ok(outputs)
    }

    /// Returns false if a Verify step found broken frames.
    fn run_steps(&self, steps: &[Step], meter: &mut Meter, stage: &str, details: &mut JobDetails) -> Result<bool, JobError> {
        for step in steps {
            if self.canceled() {
                return Err(JobError::Canceled);
            }
            match step {
                Step::Write { path, contents } => {
                    std::fs::write(path, contents).map_err(|e| JobError::Failed(e.to_string()))?;
                    details.log.push_str(&format!("\n== wrote {} ==\n{contents}", path.display()));
                }
                Step::Run { label, cmd, out_duration, weight, .. } => {
                    self.run_cmd(cmd, *out_duration, *weight, meter, stage, label, details)?;
                    meter.done += weight;
                }
                Step::Verify { path, around } => {
                    meter.report(step_weight(step), 0.0, "Checking the result");
                    if !self.verify(path, around, details)? {
                        return Ok(false);
                    }
                    meter.done += step_weight(step);
                }
            }
        }
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_cmd(
        &self,
        cmd: &FfmpegCommand,
        out_duration: f64,
        weight: f64,
        meter: &mut Meter,
        stage: &str,
        label: &str,
        details: &mut JobDetails,
    ) -> Result<(), JobError> {
        details.commands.push(cmd.display(&self.tools.ffmpeg.to_string_lossy()));
        details.log.push_str(&format!("\n== {label} ==\n"));
        meter.report(weight, 0.0, stage);
        let res = runner::run(self.tools, cmd, out_duration, self.cancel, &mut |f| meter.report(weight, f, stage));
        match res {
            Ok(log) => {
                details.log.push_str(&log.stderr);
                Ok(())
            }
            Err(RunError::Canceled) => Err(JobError::Canceled),
            Err(RunError::Failed { log, code }) => {
                details.log.push_str(&log.stderr);
                details.log.push_str(&format!("\n(ffmpeg exited with {})\n", code.map(|c| c.to_string()).unwrap_or("a signal".into())));
                Err(JobError::Failed(explain(&log.stderr)))
            }
            Err(RunError::Spawn(e)) => Err(JobError::Failed(format!("Couldn't start ffmpeg: {e}"))),
        }
    }

    /// Decode around each junction; any decoder error means a bad splice.
    fn verify(&self, path: &Path, around: &[f64], details: &mut JobDetails) -> Result<bool, JobError> {
        for &t in around {
            let cmd = FfmpegCommand::new("-")
                .input(Input::new(path).args(["-ss".to_string(), format!("{:.6}", (t - 1.0).max(0.0)), "-t".into(), "2.0".into()]))
                .args(["-map", "0:v:0", "-f", "null"]);
            details.commands.push(cmd.display(&self.tools.ffmpeg.to_string_lossy()));
            let cmd = FfmpegCommand { output_args: [vec!["-xerror".to_string()], cmd.output_args.clone()].concat(), ..cmd };
            match runner::run(self.tools, &cmd, 2.0, self.cancel, &mut |_| {}) {
                Ok(log) => {
                    let bad = log.stderr.lines().any(|l| {
                        let l = l.to_ascii_lowercase();
                        l.contains("error") || l.contains("corrupt") || l.contains("missing reference") || l.contains("could not find ref")
                    });
                    if bad {
                        details.log.push_str(&format!("\n== check at {t:.3}s found problems ==\n{}", log.stderr));
                        return Ok(false);
                    }
                }
                Err(RunError::Canceled) => return Err(JobError::Canceled),
                Err(RunError::Failed { log, .. }) => {
                    details.log.push_str(&format!("\n== check at {t:.3}s failed ==\n{}", log.stderr));
                    return Ok(false);
                }
                Err(RunError::Spawn(e)) => return Err(JobError::Failed(e.to_string())),
            }
        }
        Ok(true)
    }

    /// Fit to size: if the encode came out over the target, run the second
    /// pass again at a corrected bitrate (at most twice).
    fn enforce_size(&self, planner: &Planner, oi: usize, out: &OutputPlan, meter: &mut Meter, stage: &str, details: &mut JobDetails) -> Result<(), JobError> {
        let fit = out.fit.as_ref().expect("fit plan");
        let mut kbps = fit.settings.video_kbps;
        for attempt in 1..=2 {
            let actual = std::fs::metadata(&out.temp_path).map(|m| m.len() as f64).unwrap_or(0.0);
            details.log.push_str(&format!(
                "\n== size check: {:.2} MB for a {:.2} MB target at {kbps} kbit/s ==\n",
                actual / 1e6,
                fit.target_bytes / 1e6
            ));
            if actual <= fit.target_bytes {
                return Ok(());
            }
            kbps = fit::corrected_video_kbps(kbps, actual, fit.target_bytes, fit.audio_bytes, fit.overhead);
            details.notes.push(format!("The first try came out a little big, so attempt {} used a lower bitrate.", attempt + 1));
            let replan = planner.plan_with(CutStyle::Smart, Some(kbps))?;
            let steps: Vec<Step> = replan.outputs[oi].steps.iter().filter(|s| matches!(s, Step::Run { second_pass: true, .. })).cloned().collect();
            let extra: f64 = steps.iter().map(step_weight).sum();
            meter.total += extra;
            self.run_steps(&steps, meter, stage, details)?;
        }
        let actual = std::fs::metadata(&out.temp_path).map(|m| m.len() as f64).unwrap_or(0.0);
        if actual > fit.target_bytes {
            return Err(JobError::Failed(format!(
                "Couldn't get the video under {:.0} MB. Try a slightly bigger size.",
                fit.target_bytes / 1e6
            )));
        }
        Ok(())
    }
}

pub fn step_weight(s: &Step) -> f64 {
    match s {
        Step::Run { weight, .. } => *weight,
        Step::Verify { around, .. } => around.len() as f64 * 0.05,
        Step::Write { .. } => 0.0,
    }
}
