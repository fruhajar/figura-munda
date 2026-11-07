//! Background work.
//!
//! Inspecting a folder of photographs takes long enough to freeze a window, so
//! both phases run on a worker thread. The session is moved to the worker and
//! moved back when it finishes, which means there is exactly one owner at any
//! moment and no shared mutable state to get wrong.

use crate::session::{Session, Stage};
use std::sync::mpsc::{self, Receiver, Sender};

pub enum Job {
    Review,
    Run,
}

/// A worker that owns the session while it is busy.
pub struct Worker {
    /// `None` while a job is in flight.
    session: Option<Session>,
    inbox: Receiver<Session>,
    outbox: Sender<Session>,
    stage: Stage,
}

impl Worker {
    pub fn new(session: Session) -> Worker {
        let (outbox, inbox) = mpsc::channel();
        Worker { session: Some(session), inbox, outbox, stage: Stage::Choose }
    }

    pub fn busy(&self) -> bool {
        self.session.is_none()
    }

    /// The session, if the worker is not holding it.
    pub fn get(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn get_mut(&mut self) -> Option<&mut Session> {
        self.session.as_mut()
    }

    /// What the worker is doing, for the progress line.
    pub fn stage(&self) -> Stage {
        match &self.session {
            Some(s) => s.stage,
            None => self.stage,
        }
    }

    pub fn start(&mut self, job: Job, ctx: egui::Context) {
        let Some(mut session) = self.session.take() else { return };
        self.stage = match job {
            Job::Review => Stage::Reviewing,
            Job::Run => Stage::Running,
        };
        let outbox = self.outbox.clone();
        std::thread::spawn(move || {
            match job {
                Job::Review => session.review_blocking(),
                Job::Run => session.run_blocking(),
            }
            let _ = outbox.send(session);
            // The window is otherwise asleep and would show stale progress.
            ctx.request_repaint();
        });
    }

    /// Take the session back if the worker has finished. Call once a frame.
    pub fn poll(&mut self) -> bool {
        if self.session.is_some() {
            return false;
        }
        match self.inbox.try_recv() {
            Ok(session) => {
                self.session = Some(session);
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_with_no_job_owns_the_session() {
        let w = Worker::new(Session::default());
        assert!(!w.busy());
        assert!(w.get().is_some());
        assert_eq!(w.stage(), Stage::Choose);
    }

    #[test]
    fn polling_an_idle_worker_does_nothing() {
        let mut w = Worker::new(Session::default());
        assert!(!w.poll());
    }
}
