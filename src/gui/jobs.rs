// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! One background job at a time (scan, build, preview, download, export), reporting to the window
//! through a channel. The window never waits on a job; it drains messages once per frame.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use reskate_music::music::{PackResult, SongInfo};

/// A decoded preview image: RGBA pixels, width, height.
pub type Pixels = (Vec<u8>, u32, u32);

pub enum Message {
    Progress { fraction: f32, text: String },
    Scanned(SongInfo),
    Built(Result<PackResult, String>),
    Preview { generation: u64, result: Result<Pixels, String> },
    Downloaded(Result<PathBuf, String>),
    Exported(Result<PathBuf, String>),
}

/// What a job's closure gets: a way to report, and its cancel flag.
#[derive(Clone)]
pub struct Reporter {
    sender: Sender<Message>,
    cancel: Arc<AtomicBool>,
}

impl Reporter {
    pub fn send(&self, message: Message) {
        let _ = self.sender.send(message);
    }

    pub fn progress(&self, fraction: f32, text: impl Into<String>) {
        self.send(Message::Progress { fraction, text: text.into() });
    }

    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

struct Running {
    label: String,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

pub struct Jobs {
    running: Option<Running>,
    sender: Sender<Message>,
    receiver: Receiver<Message>,
    pub fraction: f32,
    pub text: String,
}

impl Jobs {
    pub fn new() -> Self {
        let (sender, receiver) = channel();
        Jobs { running: None, sender, receiver, fraction: 0.0, text: String::new() }
    }

    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// The running job's label ("Scanning", "Building", ...).
    pub fn label(&self) -> Option<&str> {
        self.running.as_ref().map(|job| job.label.as_str())
    }

    /// Starts `work` on a worker thread. Callers check `busy()` first; a second job waits for the
    /// first to finish.
    pub fn start(&mut self, label: &str, work: impl FnOnce(Reporter) + Send + 'static) {
        if let Some(previous) = self.running.take() {
            let _ = previous.handle.join();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let reporter = Reporter { sender: self.sender.clone(), cancel: Arc::clone(&cancel) };
        let handle = std::thread::spawn(move || work(reporter));
        self.fraction = 0.0;
        self.text.clear();
        self.running = Some(Running { label: label.into(), cancel, handle });
    }

    pub fn cancel(&self) {
        if let Some(job) = &self.running {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Messages since the last call; progress is absorbed here. Also notices a finished job.
    pub fn drain(&mut self) -> Vec<Message> {
        let mut messages = Vec::new();
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                Message::Progress { fraction, text } => {
                    self.fraction = fraction;
                    self.text = text;
                }
                other => messages.push(other),
            }
        }
        if self.running.as_ref().is_some_and(|job| job.handle.is_finished()) {
            let job = self.running.take().expect("checked above");
            let _ = job.handle.join();
            // Anything sent just before the thread ended.
            messages.extend(self.receiver.try_iter().filter(|message| !matches!(message, Message::Progress { .. })));
        }
        messages
    }

    /// Cancels and waits for the running job (on exit).
    pub fn shutdown(&mut self) {
        self.cancel();
        if let Some(job) = self.running.take() {
            let _ = job.handle.join();
        }
    }
}
