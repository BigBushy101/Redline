//! Reacting to the queue as jobs end: what to tell the user, keeping the PC awake, and the
//! one-shot "when everything's done" action.

use std::path::{Path, PathBuf};

use crate::jobs::{Job, JobStatus};
use crate::system::{self, AfterQueue};

/// A desktop notification's text.
#[derive(Debug, PartialEq)]
pub struct Announcement {
    pub title: String,
    pub body: String,
}

/// What to say about jobs that just ended (title, status), or `None` if they were all cancelled.
/// `remaining` is how many jobs are still queued or running.
pub fn announcement(ended: &[(String, JobStatus)], remaining: usize) -> Option<Announcement> {
    let ended: Vec<_> = ended
        .iter()
        .filter(|(_, s)| !matches!(s, JobStatus::Cancelled))
        .collect();
    if ended.is_empty() {
        return None;
    }
    let failed = ended.iter().filter(|(_, s)| matches!(s, JobStatus::Failed(_))).count();
    let (title, body) = match ended.as_slice() {
        [(name, status)] => (
            match status {
                JobStatus::Failed(_) => "Failed".to_owned(),
                JobStatus::Skipped(why) => why.clone(),
                _ => "Finished".to_owned(),
            },
            name.clone(),
        ),
        many => (
            format!("{} jobs ended", many.len()),
            if failed > 0 {
                format!("{} finished, {failed} failed", many.len() - failed)
            } else {
                "All finished".to_owned()
            },
        ),
    };
    let body = if remaining > 0 {
        format!("{body} · {remaining} still in the queue")
    } else {
        body
    };
    Some(Announcement { title, body })
}

/// The after-queue action that's due, with what it needs.
#[derive(Debug, PartialEq)]
pub enum AfterQueueStep {
    OpenFolder(PathBuf),
    Sleep,
    ShutDown,
}

#[derive(Default)]
pub struct QueueWatch {
    /// One-shot action once the queue empties.
    pub after_queue: AfterQueue,
    /// A job finished successfully since the queue was last empty.
    finished_ok: bool,
    /// Last output folder, for "Open folder when done".
    last_output: Option<PathBuf>,
    /// The PC is being kept awake for active jobs.
    awake: bool,
}

impl QueueWatch {
    /// Notes a job that just ended.
    pub fn job_ended(&mut self, job: &Job) {
        if matches!(job.status, JobStatus::Done(_) | JobStatus::Skipped(_)) {
            self.finished_ok = true;
            self.last_output = job.output_dir();
        }
    }

    /// Keeps the PC awake while jobs are active, and returns the after-queue action once the
    /// queue empties after a success. The action is used up.
    pub fn update(&mut self, active: usize, download_dir: &Path) -> Option<AfterQueueStep> {
        let mut step = None;
        if active == 0 && self.finished_ok {
            self.finished_ok = false;
            step = match std::mem::take(&mut self.after_queue) {
                AfterQueue::Nothing => None,
                AfterQueue::OpenFolder => Some(AfterQueueStep::OpenFolder(
                    self.last_output
                        .clone()
                        .unwrap_or_else(|| download_dir.to_path_buf()),
                )),
                AfterQueue::Sleep => Some(AfterQueueStep::Sleep),
                AfterQueue::ShutDown => Some(AfterQueueStep::ShutDown),
            };
        }
        if (active > 0) != self.awake {
            self.awake = active > 0;
            system::keep_awake(self.awake);
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcements() {
        let done = |n: &str| (n.to_owned(), JobStatus::Done(None));
        assert_eq!(announcement(&[("x".into(), JobStatus::Cancelled)], 0), None);
        assert_eq!(
            announcement(&[done("Song")], 0),
            Some(Announcement { title: "Finished".into(), body: "Song".into() })
        );
        assert_eq!(
            announcement(&[done("a"), ("b".into(), JobStatus::Failed("x".into()))], 2),
            Some(Announcement {
                title: "2 jobs ended".into(),
                body: "1 finished, 1 failed · 2 still in the queue".into()
            })
        );
    }

    #[test]
    fn after_queue_fires_once_after_success() {
        let mut w = QueueWatch {
            after_queue: AfterQueue::OpenFolder,
            ..Default::default()
        };
        let dir = Path::new("dl");
        assert_eq!(w.update(0, dir), None, "nothing finished yet");
        w.finished_ok = true;
        assert_eq!(w.update(0, dir), Some(AfterQueueStep::OpenFolder(dir.into())));
        assert_eq!(w.after_queue, AfterQueue::Nothing);
        w.finished_ok = true;
        assert_eq!(w.update(0, dir), None, "used up");
    }
}
