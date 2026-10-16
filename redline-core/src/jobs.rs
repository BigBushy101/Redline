//! Download / conversion queue. Jobs run on worker threads and report back over a channel
//! that the UI drains once per frame.

use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::settings;
use crate::{Wake, no_wake};
use crate::tools::convert::{ConcatRequest, ConvertRequest};
use crate::tools::ytdlp::DownloadRequest;
use crate::tools::{self, ToolPaths};

pub type JobId = u64;

#[derive(Clone, Serialize, Deserialize)]
pub enum JobSpec {
    Download(DownloadRequest),
    Convert(ConvertRequest),
    Concat(ConcatRequest),
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum JobStatus {
    Queued,
    Running { progress: Option<f32>, detail: String },
    Processing(String),
    Done(Option<PathBuf>),
    /// Finished without doing anything, e.g. the link is in the download archive.
    Skipped(String),
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

#[derive(Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub title: String,
    pub subtitle: String,
    pub spec: JobSpec,
    pub status: JobStatus,
    /// Don't start before this time (Unix seconds).
    #[serde(default)]
    pub not_before: Option<i64>,
    #[serde(skip)]
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
                JobSpec::Concat(r) => r
                    .out_dir
                    .clone()
                    .or_else(|| r.inputs.first()?.parent().map(|p| p.to_path_buf())),
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

/// Error returned by a download whose link is already in the download archive.
#[derive(Debug)]
pub struct AlreadyDownloaded;

impl std::fmt::Display for AlreadyDownloaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("already downloaded")
    }
}

impl std::error::Error for AlreadyDownloaded {}

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
    wake: Wake,
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
            wake: no_wake(),
        }
    }

    fn send(&self, ev: JobEvent) {
        let _ = self.tx.send(ev);
        (self.wake)();
    }
}

fn queue_path() -> PathBuf {
    settings::data_dir().join("queue.json")
}

pub struct JobManager {
    pub jobs: Vec<Job>,
    next_id: JobId,
    running: usize,
    tx: Sender<JobEvent>,
    rx: Receiver<JobEvent>,
    /// The job list changed in a way worth saving (not progress ticks).
    dirty: bool,
    /// Jobs that finished since the last `take_finished`.
    finished: Vec<JobId>,
    wake: Wake,
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
            dirty: false,
            finished: Vec::new(),
            wake: no_wake(),
        }
    }

    /// Restores the queue saved by the last session. Jobs that were mid-run start over.
    pub fn load() -> Self {
        let mut m = Self::new();
        let jobs: Vec<Job> = std::fs::read_to_string(queue_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        m.set_jobs(jobs);
        m
    }

    /// Called by workers whenever a job reports progress or ends.
    pub fn set_wake(&mut self, wake: Wake) {
        self.wake = wake;
    }

    fn set_jobs(&mut self, mut jobs: Vec<Job>) {
        for job in &mut jobs {
            if matches!(job.status, JobStatus::Running { .. } | JobStatus::Processing(_)) {
                job.status = JobStatus::Queued;
            }
        }
        self.next_id = jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
        self.jobs = jobs;
    }

    /// Writes the queue to disk if it changed since the last save.
    pub fn save_if_dirty(&mut self) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        let path = queue_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string(&self.jobs) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Ids of jobs that finished (any outcome) since the last call.
    pub fn take_finished(&mut self) -> Vec<JobId> {
        std::mem::take(&mut self.finished)
    }

    pub fn get(&self, id: JobId) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn push(&mut self, title: String, subtitle: String, spec: JobSpec) {
        self.push_at(title, subtitle, spec, None);
    }

    /// Queues a job that waits until `not_before` (Unix seconds) to start.
    pub fn push_at(&mut self, title: String, subtitle: String, spec: JobSpec, not_before: Option<i64>) {
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(Job {
            id,
            title,
            subtitle,
            spec,
            status: JobStatus::Queued,
            not_before,
            cancel: Arc::new(CancelHandle::default()),
        });
        self.dirty = true;
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
                JobStatus::Queued => {
                    job.status = JobStatus::Cancelled;
                    self.dirty = true;
                }
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
            self.dirty = true;
        }
    }

    /// Retries a download without the archive check ("Download anyway").
    pub fn retry_ignoring_archive(&mut self, id: JobId) {
        if let Some(job) = self.find(id)
            && let JobSpec::Download(req) = &mut job.spec
        {
            req.archive = None;
        }
        self.retry(id);
    }

    pub fn remove(&mut self, id: JobId) {
        self.jobs
            .retain(|j| j.id != id || j.status.is_active());
        self.dirty = true;
    }

    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| j.status.is_active());
        self.dirty = true;
    }

    pub fn cancel_all(&mut self) {
        let ids: Vec<_> = self.jobs.iter().map(|j| j.id).collect();
        for id in ids {
            self.cancel(id);
        }
    }

    /// Applies worker events and starts queued jobs up to the concurrency limit.
    /// Returns how long until the next scheduled job is due, so the caller can check back then.
    pub fn pump(&mut self, tools: &ToolPaths, concurrency: usize) -> Option<std::time::Duration> {
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
                            Err(e) if e.is::<AlreadyDownloaded>() => {
                                JobStatus::Skipped("Already downloaded".into())
                            }
                            Err(e) => JobStatus::Failed(format!("{e:#}")),
                        };
                        self.finished.push(id);
                        self.dirty = true;
                    }
                }
            }
        }

        let now = chrono::Utc::now().timestamp();
        let due = |j: &Job| j.not_before.is_none_or(|t| t <= now);
        let recheck = self
            .jobs
            .iter()
            .filter(|j| j.status == JobStatus::Queued && !due(j))
            .filter_map(|j| j.not_before)
            .min()
            .map(|next| std::time::Duration::from_secs((next - now).clamp(1, 60) as u64));
        while self.running < concurrency.max(1) {
            let Some(job) = self
                .jobs
                .iter_mut()
                .find(|j| j.status == JobStatus::Queued && due(j))
            else {
                break;
            };
            job.not_before = None;
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
                wake: self.wake.clone(),
            };
            thread::spawn(move || {
                let result = match &spec {
                    JobSpec::Download(req) => tools::ytdlp::download(&tools, req, &cancel, &reporter),
                    JobSpec::Convert(req) => tools::convert::run(&tools, req, &cancel, &reporter),
                    JobSpec::Concat(req) => tools::convert::run_concat(&tools, req, &cancel, &reporter),
                };
                reporter.send(JobEvent::Finished(reporter.id, result));
            });
        }
        recheck
    }

    fn find(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduled_jobs_wait_until_due() {
        let mut m = JobManager::new();
        let now = chrono::Utc::now().timestamp();
        let spec = || JobSpec::Download(DownloadRequest::default());
        m.push_at("later".into(), String::new(), spec(), Some(now + 3600));
        m.push_at("due".into(), String::new(), spec(), Some(now - 1));
        let recheck = m.pump(&ToolPaths::default(), 4);
        assert!(recheck.is_some_and(|d| d.as_secs() >= 1 && d.as_secs() <= 60), "asks to be woken for the later job");
        assert!(m.jobs[0].status == JobStatus::Queued, "not due yet");
        assert!(m.jobs[0].not_before.is_some());
        assert!(m.jobs[1].status != JobStatus::Queued, "due job started");
        assert_eq!(m.jobs[1].not_before, None);
        assert_eq!(m.active_count(), 2, "scheduled jobs count as active");
    }

    #[test]
    fn queue_round_trips_and_resumes() {
        let mut m = JobManager::new();
        m.push(
            "a".into(),
            "sub".into(),
            JobSpec::Download(DownloadRequest {
                url: "https://example.com/v".into(),
                ..Default::default()
            }),
        );
        m.jobs[0].status = JobStatus::Running {
            progress: Some(0.5),
            detail: String::new(),
        };
        let json = serde_json::to_string(&m.jobs).unwrap();

        let mut back = JobManager::new();
        back.set_jobs(serde_json::from_str(&json).unwrap());
        assert_eq!(back.jobs.len(), 1);
        assert!(back.jobs[0].status == JobStatus::Queued);
        assert_eq!(back.next_id, 2);
        let JobSpec::Download(r) = &back.jobs[0].spec else {
            panic!("wrong spec")
        };
        assert_eq!(r.url, "https://example.com/v");
    }
}
