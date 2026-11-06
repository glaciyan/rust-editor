//! The job queue. The GUI sends commands and drains events every frame; it
//! never waits on ffmpeg.

use crate::estimate::{self, Calibration};
use crate::job::{Job, JobDetails, JobError, JobId, JobOutput};
use crate::request::ExportRequest;
use crossbeam_channel::{unbounded, Receiver, Sender};
use cutline_ffmpeg::{MediaInfo, Tools};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub enum EngineEvent {
    Queued { job: JobId, outputs: Vec<PathBuf> },
    Started { job: JobId },
    Progress { job: JobId, fraction: f64, eta_secs: Option<f64>, stage: String },
    Finished { job: JobId, outputs: Vec<JobOutput>, details: JobDetails, seconds: f64 },
    Failed { job: JobId, message: String, details: JobDetails },
    Canceled { job: JobId },
    Calibrated { key: u64, calibration: Option<Calibration> },
}

struct Queued {
    id: JobId,
    req: ExportRequest,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Shared {
    queue: VecDeque<Queued>,
    running: Option<(JobId, Arc<AtomicBool>)>,
}

pub struct Engine {
    pub tools: Arc<Tools>,
    events_tx: Sender<EngineEvent>,
    events: Receiver<EngineEvent>,
    shared: Arc<(Mutex<Shared>, std::sync::Condvar)>,
    next_id: AtomicU64,
    calibrating: Mutex<Option<Arc<AtomicBool>>>,
    /// Called after each event is sent, to wake the UI.
    waker: Arc<Mutex<Option<Box<dyn Fn() + Send>>>>,
}

impl Engine {
    pub fn new(tools: Tools) -> Engine {
        let (events_tx, events) = unbounded();
        let shared: Arc<(Mutex<Shared>, std::sync::Condvar)> = Arc::default();
        let tools = Arc::new(tools);
        let waker: Arc<Mutex<Option<Box<dyn Fn() + Send>>>> = Arc::default();
        {
            let shared = shared.clone();
            let tools = tools.clone();
            let tx = events_tx.clone();
            let waker = waker.clone();
            std::thread::Builder::new()
                .name("cutline-jobs".into())
                .spawn(move || worker(tools, shared, tx, waker))
                .expect("spawn worker");
        }
        Engine { tools, events_tx, events, shared, next_id: AtomicU64::new(1), calibrating: Mutex::new(None), waker }
    }

    /// Wake the UI whenever an event arrives.
    pub fn set_waker(&self, f: impl Fn() + Send + 'static) {
        *self.waker.lock().unwrap() = Some(Box::new(f));
    }

    pub fn poll(&self) -> Vec<EngineEvent> {
        self.events.try_iter().collect()
    }

    pub fn submit(&self, req: ExportRequest, outputs: Vec<PathBuf>) -> JobId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap().queue.push_back(Queued { id, req, cancel: Arc::new(AtomicBool::new(false)) });
        cv.notify_all();
        let _ = self.events_tx.send(EngineEvent::Queued { job: id, outputs });
        id
    }

    pub fn cancel(&self, job: JobId) {
        let (lock, _) = &*self.shared;
        let mut s = lock.lock().unwrap();
        if let Some((id, flag)) = &s.running {
            if *id == job {
                flag.store(true, Ordering::Relaxed);
                return;
            }
        }
        let before = s.queue.len();
        s.queue.retain(|q| q.id != job);
        if s.queue.len() != before {
            let _ = self.events_tx.send(EngineEvent::Canceled { job });
        }
    }

    pub fn cancel_all(&self) {
        let (lock, _) = &*self.shared;
        let mut s = lock.lock().unwrap();
        s.queue.clear();
        if let Some((_, flag)) = &s.running {
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn busy(&self) -> bool {
        let (lock, _) = &*self.shared;
        let s = lock.lock().unwrap();
        s.running.is_some() || !s.queue.is_empty()
    }

    /// Start a sample encode in the background; a newer request cancels an older one.
    pub fn calibrate(&self, key: u64, req: ExportRequest, sources: HashMap<PathBuf, MediaInfo>) {
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(old) = self.calibrating.lock().unwrap().replace(cancel.clone()) {
            old.store(true, Ordering::Relaxed);
        }
        let tools = self.tools.clone();
        let tx = self.events_tx.clone();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let c = estimate::calibrate(&tools, &req, &sources, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                let _ = tx.send(EngineEvent::Calibrated { key, calibration: c });
                if let Some(w) = &*waker.lock().unwrap() {
                    w();
                }
            }
        });
    }
}

fn worker(
    tools: Arc<Tools>,
    shared: Arc<(Mutex<Shared>, std::sync::Condvar)>,
    tx: Sender<EngineEvent>,
    waker: Arc<Mutex<Option<Box<dyn Fn() + Send>>>>,
) {
    let wake = || {
        if let Some(w) = &*waker.lock().unwrap() {
            w();
        }
    };
    loop {
        let next = {
            let (lock, cv) = &*shared;
            let mut s = lock.lock().unwrap();
            loop {
                if let Some(q) = s.queue.pop_front() {
                    s.running = Some((q.id, q.cancel.clone()));
                    break q;
                }
                s = cv.wait(s).unwrap();
            }
        };
        let _ = tx.send(EngineEvent::Started { job: next.id });
        wake();
        let started = std::time::Instant::now();
        let job = Job { id: next.id, tools: &tools, req: &next.req, cancel: &next.cancel };
        let mut details = JobDetails::default();
        let mut last_sent = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let result = job.run(
            &mut |p| {
                if last_sent.elapsed().as_millis() >= 100 {
                    last_sent = std::time::Instant::now();
                    let _ = tx.send(EngineEvent::Progress { job: next.id, fraction: p.fraction, eta_secs: p.eta_secs, stage: p.stage });
                    wake();
                }
            },
            &mut details,
        );
        let ev = match result {
            Ok(outputs) => EngineEvent::Finished { job: next.id, outputs, details, seconds: started.elapsed().as_secs_f64() },
            Err(JobError::Canceled) => EngineEvent::Canceled { job: next.id },
            Err(e) => EngineEvent::Failed { job: next.id, message: e.to_string(), details },
        };
        {
            let (lock, _) = &*shared;
            lock.lock().unwrap().running = None;
        }
        let _ = tx.send(ev);
        wake();
    }
}
