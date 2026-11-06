//! `cutline-cli`: the engine without a window. Handy for scripts and tests.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use cutline_core::{format, Profile, Segment, Timestamp};
use cutline_engine::plan::{Facts, Planner, Step};
use cutline_engine::request::{default_name, ExportRequest};
use cutline_engine::store::ProfileStore;
use cutline_engine::{estimate, Engine, EngineEvent};
use cutline_ffmpeg::{probe, Tools};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "cutline-cli", version, about = "Cut videos without the window.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show what Cutline sees in a file.
    Probe { file: PathBuf },
    /// List export profiles.
    Profiles,
    /// Export parts of one or more files.
    Export {
        /// Input files, in timeline order.
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Part to keep, as START-END in seconds or m:ss (repeatable). Applies to the
        /// file given before it with --file-index (default: the first file).
        #[arg(long = "keep", value_name = "START-END")]
        keep: Vec<String>,
        /// Profile id (see `profiles`). Default: original.
        #[arg(long, default_value = "original")]
        profile: String,
        /// For fit to size: target in MB.
        #[arg(long)]
        size: Option<f64>,
        /// Output folder (default: next to the first file).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Output name without extension (default: "<name> (cut)").
        #[arg(long)]
        name: Option<String>,
        /// One file per part.
        #[arg(long)]
        separate: bool,
        /// Replace existing files.
        #[arg(long)]
        overwrite: bool,
        /// Audio tracks to keep, by position (default: the first).
        #[arg(long, value_delimiter = ',')]
        audio: Option<Vec<usize>>,
        /// Print the plan and estimates without running anything.
        #[arg(long)]
        plan: bool,
    },
}

fn parse_time(s: &str) -> Result<f64> {
    let mut total = 0.0;
    for part in s.trim().split(':') {
        total = total * 60.0 + part.parse::<f64>().with_context(|| format!("bad time {s:?}"))?;
    }
    Ok(total)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let tools = Tools::locate().map_err(anyhow::Error::msg)?;
    match cli.cmd {
        Cmd::Probe { file } => {
            let info = probe::probe(&tools, &file)?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        Cmd::Profiles => {
            println!("original  Original quality — {}", Profile::original().description);
            println!("fit       Fit to size (use --size MB)");
            for p in ProfileStore::load().presets() {
                println!("{:<9} {} — {}", p.id, p.name, p.description);
            }
        }
        Cmd::Export { files, keep, profile, size, out, name, separate, overwrite, audio, plan } => {
            let mut sources = std::collections::HashMap::new();
            for f in &files {
                let f = std::fs::canonicalize(f).with_context(|| format!("can't find {}", f.display()))?;
                sources.insert(f.clone(), probe::probe(&tools, &f)?);
            }
            let canon: Vec<PathBuf> = files.iter().map(|f| std::fs::canonicalize(f).unwrap()).collect();
            let mut segments = vec![];
            if keep.is_empty() {
                for f in &canon {
                    let d = sources[f].duration;
                    segments.push(Segment { clip: 0, path: f.clone(), start: Timestamp::ZERO, end: Timestamp::from_secs(d) });
                }
            }
            for k in &keep {
                let (file_ix, range) = match k.split_once('@') {
                    Some((r, ix)) => (ix.parse::<usize>()?, r),
                    None => (0, k.as_str()),
                };
                let (a, b) = range.split_once('-').context("--keep needs START-END")?;
                let f = canon.get(file_ix).context("--keep RANGE@N: no such file")?;
                let fps = sources[f].video.as_ref().map(|v| v.fps).unwrap_or(0.0);
                segments.push(Segment {
                    clip: file_ix as u64,
                    path: f.clone(),
                    start: Timestamp::from_secs(parse_time(a)?).snap_to_frame(fps),
                    end: Timestamp::from_secs(parse_time(b)?.min(sources[f].duration)).snap_to_frame(fps),
                });
            }
            let profile = match profile.as_str() {
                "original" => Profile::original(),
                "fit" => Profile::fit(size.context("fit needs --size MB")?),
                id => ProfileStore::load().presets().into_iter().find(|p| p.id == id).with_context(|| format!("no profile {id:?}"))?,
            };
            let first = canon[0].clone();
            let req = ExportRequest {
                segments,
                profile,
                output_dir: out.unwrap_or_else(|| first.parent().unwrap().to_path_buf()),
                name: name.unwrap_or_else(|| default_name(&first)),
                separate,
                audio_tracks: audio.unwrap_or_else(|| if sources[&first].audio.is_empty() { vec![] } else { vec![0] }),
                overwrite,
            };
            let e = match req.profile.video.rate {
                cutline_core::RateControl::TargetSize(mb) if !req.profile.is_copy() => match estimate::fit(&req, &sources, &tools.caps, mb, None) {
                    estimate::FitEstimate::Original(e) | estimate::FitEstimate::Encode(e) => e,
                    estimate::FitEstimate::TooSmall { min_mb } => bail!("Too small for this length. Try at least {min_mb} MB."),
                },
                _ if req.profile.is_copy() => estimate::original(&req, &sources, &tools.caps, None),
                _ => estimate::encode(&req, &sources, None),
            };
            eprintln!(
                "{}{} · {} · {}",
                e.quality.map(|q| format!("{} · ", q.label())).unwrap_or_default(),
                format::length(req.total_duration()),
                format::about_size(e.bytes),
                format::ready_in(e.seconds)
            );
            if plan {
                let facts = Facts { sources: sources.clone(), packets: vec![] };
                let tmp = req.output_dir.join(".cutline-plan");
                let p = Planner { req: &req, facts: &facts, caps: &tools.caps, tmp: &tmp, pass_log_dir: &std::env::temp_dir() }.plan()?;
                for n in &p.notes {
                    println!("# {n}");
                }
                for o in &p.outputs {
                    println!("# → {}", o.final_path.display());
                    for s in &o.steps {
                        match s {
                            Step::Run { label, cmd, .. } => println!("# {label}\n{}", cmd.display("ffmpeg")),
                            Step::Write { path, contents } => println!("# write {}\n{contents}", path.display()),
                            Step::Verify { around, .. } => println!("# verify junctions at {around:?}"),
                        }
                    }
                }
                println!("# (copy exports read keyframes at run time; the real head/copy/tail split appears in Details)");
                return Ok(());
            }
            let outputs = req.output_paths(&req.extension(sources.get(&first)));
            let engine = Engine::new(tools);
            let id = engine.submit(req, outputs);
            loop {
                for ev in engine.poll() {
                    match ev {
                        EngineEvent::Progress { job, fraction, eta_secs, stage } if job == id => {
                            eprint!(
                                "\r{stage} · {:>3.0}%{}        ",
                                fraction * 100.0,
                                eta_secs.map(|s| format!(" · {}", format::time_left(s))).unwrap_or_default()
                            );
                        }
                        EngineEvent::Finished { outputs, details, seconds, .. } => {
                            eprintln!();
                            for n in details.notes {
                                eprintln!("note: {n}");
                            }
                            if std::env::var_os("CUTLINE_DETAILS").is_some() {
                                for c in &details.commands {
                                    eprintln!("$ {c}");
                                }
                                eprintln!("{}", details.log);
                            }
                            for o in outputs {
                                println!("Saved · {} · {} ({:.1} s)", format::size(o.bytes), o.path.display(), seconds);
                            }
                            return Ok(());
                        }
                        EngineEvent::Failed { message, details, .. } => {
                            eprintln!();
                            for c in &details.commands {
                                eprintln!("$ {c}");
                            }
                            eprintln!("{}", details.log);
                            bail!(message);
                        }
                        EngineEvent::Canceled { .. } => bail!("canceled"),
                        _ => {}
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    Ok(())
}
