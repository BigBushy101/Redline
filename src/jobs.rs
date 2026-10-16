//! Download / conversion queue. Jobs run on worker threads and report back over a channel
//! that the UI drains once per frame.

use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui;

use crate::tools::convert::ConvertRequest;
use crate::tools::ytdlp::DownloadRequest;
use crate::tools::{self, ToolPaths};

pub type JobId = u64;

#[derive(Clone)]
pub enum JobSpec {
    Download(DownloadRequest),
    Convert(ConvertRequest),
}

#[derive(Clone, PartialEq)]
pub enum JobStatus {
    Queued,
    Running { progress: Option<f32>, detail: String },
    Processing(String),
    Done(Option<PathBuf>),
    Failed(String),
    Cancelled,
}

impl JobStatus {
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            JobStatus::Queued | JobStatus::Running { .. } | JobStatus::Processing(_)
        )
    }

    pub fn is_finished(&self) -> bool {
        !self.is_active()
    }
}

pub struct Job {
    pub id: JobId,
    pub title: String,
    pub subtitle: String,
    pub spec: JobSpec,
    pub status: JobStatus,
    cancel: Arc<CancelHandle>,
}

impl Job {
    pub fn is_download(&self) -> bool {
        matches!(self.spec, JobSpec::Download(_))
    }

    pub fn output_dir(&self) -> Option<PathBuf> {
        match &self.status {
            JobStatus::Done(Some(p)) => p.parent().map(|p| p.to_path_buf()),
            _ => match &self.spec {
                JobSpec::Download(r) => Some(r.out_dir.clone()),
                JobSpec::Convert(r) => r
                    .out_dir
                    .clone()
                    .or_else(|| r.input.parent().map(|p| p.to_path_buf())),
            },
        }
    }
}

/// Lets the UI kill a job's running external process.
#[derive(Default)]
pub struct CancelHandle {
    cancelled: AtomicBool,
    child: Mutex<Option<Child>>,
}

impl CancelHandle {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            tools::kill_tree(child);
        }
    }

    /// Registers the running process; kills it straight away if cancel already happened.
    pub fn attach(&self, mut child: Child) {
        if self.is_cancelled() {
            tools::kill_tree(&mut child);
        }
        *self.child.lock().unwrap() = Some(child);
    }

    /// Polls rather than blocking in `Child::wait` so `cancel()` can still take the lock.
    pub fn wait(&self) -> std::io::Result<std::process::ExitStatus> {
        loop {
            {
                let mut guard = self.child.lock().unwrap();
                let child = guard.as_mut().expect("wait() called without a child");
                if let Some(status) = child.try_wait()? {
                    *guard = None;
                    return Ok(status);
                }
            }
            thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// Error returned by a job that was cancelled by the user.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

enum JobEvent {
    Status(JobId, JobStatus),
    Title(JobId, String),
    Finished(JobId, anyhow::Result<Option<PathBuf>>),
}

/// Handed to a worker so it can report progress.
#[derive(Clone)]
pub struct Reporter {
    id: JobId,
    tx: Sender<JobEvent>,
    ctx: egui::Context,
}

impl Reporter {
    pub fn progress(&self, progress: Option<f32>, detail: impl Into<String>) {
        self.send(JobEvent::Status(
            self.id,
            JobStatus::Running {
                progress,
                detail: detail.into(),
            },
        ));
    }

    pub fn processing(&self, msg: impl Into<String>) {
        self.send(JobEvent::Status(self.id, JobStatus::Processing(msg.into())));
    }

    pub fn title(&self, title: impl Into<String>) {
        self.send(JobEvent::Title(self.id, title.into()));
    }

    #[cfg(test)]
    pub fn for_test() -> Self {
        Self {
            id: 0,
            tx: channel().0,
            ctx: egui::Context::default(),
        }
    }

    fn send(&self, ev: JobEvent) {
        let _ = self.tx.send(ev);
        self.ctx.request_repaint();
    }
}

pub struct JobManager {
    pub jobs: Vec<Job>,
    next_id: JobId,
    running: usize,
    tx: Sender<JobEvent>,
    rx: Receiver<JobEvent>,
}

impl JobManager {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            jobs: Vec::new(),
            next_id: 1,
            running: 0,
            tx,
            rx,
        }
    }

    pub fn push(&mut self, title: String, subtitle: String, spec: JobSpec) {
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(Job {
            id,
            title,
            subtitle,
            spec,
            status: JobStatus::Queued,
            cancel: Arc::new(CancelHandle::default()),
        });
    }

    pub fn active_count(&self) -> usize {
        self.jobs.iter().filter(|j| j.status.is_active()).count()
    }

    /// The job shown in the bottom "now" bar: the oldest running one, else the newest.
    pub fn current(&self) -> Option<&Job> {
        self.jobs
            .iter()
            .find(|j| matches!(j.status, JobStatus::Running { .. } | JobStatus::Processing(_)))
            .or_else(|| self.jobs.iter().find(|j| j.status == JobStatus::Queued))
            .or_else(|| self.jobs.last())
    }

    pub fn cancel(&mut self, id: JobId) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            match job.status {
                JobStatus::Queued => job.status = JobStatus::Cancelled,
                JobStatus::Running { .. } | JobStatus::Processing(_) => job.cancel.cancel(),
                _ => {}
            }
        }
    }

    pub fn retry(&mut self, id: JobId) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id)
            && job.status.is_finished()
        {
            job.status = JobStatus::Queued;
            job.cancel = Arc::new(CancelHandle::default());
        }
    }

    pub fn remove(&mut self, id: JobId) {
        self.jobs
            .retain(|j| j.id != id || j.status.is_active());
    }

    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| j.status.is_active());
    }

    pub fn cancel_all(&mut self) {
        let ids: Vec<_> = self.jobs.iter().map(|j| j.id).collect();
        for id in ids {
            self.cancel(id);
        }
    }

    /// Applies worker events and starts queued jobs up to the concurrency limit.
    pub fn pump(&mut self, ctx: &egui::Context, tools: &ToolPaths, concurrency: usize) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                JobEvent::Status(id, status) => {
                    if let Some(job) = self.find(id)
                        && job.status.is_active()
                    {
                        job.status = status;
                    }
                }
                JobEvent::Title(id, title) => {
                    if let Some(job) = self.find(id) {
                        job.title = title;
                    }
                }
                JobEvent::Finished(id, result) => {
                    self.running = self.running.saturating_sub(1);
                    if let Some(job) = self.find(id) {
                        job.status = match result {
                            Ok(path) => JobStatus::Done(path),
                            Err(e) if e.is::<Cancelled>() || job.cancel.is_cancelled() => {
                                JobStatus::Cancelled
                            }
                            Err(e) => JobStatus::Failed(format!("{e:#}")),
                        };
                    }
                }
            }
        }

        while self.running < concurrency.max(1) {
            let Some(job) = self.jobs.iter_mut().find(|j| j.status == JobStatus::Queued) else {
                break;
            };
            job.status = JobStatus::Running {
                progress: None,
                detail: "Starting…".into(),
            };
            self.running += 1;

            let spec = job.spec.clone();
            let cancel = job.cancel.clone();
            let tools = tools.clone();
            let reporter = Reporter {
                id: job.id,
                tx: self.tx.clone(),
                ctx: ctx.clone(),
            };
            thread::spawn(move || {
                let result = match &spec {
                    JobSpec::Download(req) => tools::ytdlp::download(&tools, req, &cancel, &reporter),
                    JobSpec::Convert(req) => tools::convert::run(&tools, req, &cancel, &reporter),
                };
                reporter.send(JobEvent::Finished(reporter.id, result));
            });
        }
    }

    fn find(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }
}
