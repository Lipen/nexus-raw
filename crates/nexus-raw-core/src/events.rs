//! Progress event stream with byte coalescing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::{mpsc, Mutex};

use crate::diff::Action;

/// Transfer direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dir {
    Up,
    Down,
}

impl Dir {
    pub fn as_state(self) -> &'static str {
        match self {
            Dir::Up => "uploading",
            Dir::Down => "downloading",
        }
    }
}

/// Operation result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub uploaded: usize,
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: Vec<String>,
}

/// Events the core emits; the NDJSON shapes are fixed by golden tests.
#[derive(Debug, Clone)]
pub enum Event {
    /// Diff plan: name lists.
    Plan {
        upload: Vec<String>,
        download: Vec<String>,
        skip: Vec<String>,
    },
    ArtifactStarted {
        name: String,
        dir: Dir,
        total: Option<u64>,
    },
    /// Coalesced: at most once per 200 ms per name.
    ArtifactBytes {
        name: String,
        dir: Dir,
        done: u64,
        total: Option<u64>,
    },
    ArtifactDone {
        name: String,
        dir: Dir,
        skipped: bool,
        done: u64,
        total: Option<u64>,
    },
    Retrying {
        name: String,
        attempt: u32,
        reason: String,
    },
    Summary(Summary),
}

impl Event {
    /// The event's JSON form; wrappers serialize it 1:1.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Event::Plan {
                upload,
                download,
                skip,
            } => serde_json::json!({
                "event": "plan",
                "upload": upload,
                "download": download,
                "skip": skip,
            }),
            Event::ArtifactStarted { name, dir, total } => serde_json::json!({
                "event": "artifact",
                "name": name,
                "state": dir.as_state(),
                "done": 0,
                "total": total,
            }),
            Event::ArtifactBytes {
                name,
                dir,
                done,
                total,
            } => serde_json::json!({
                "event": "artifact",
                "name": name,
                "state": dir.as_state(),
                "done": done,
                "total": total,
            }),
            Event::ArtifactDone {
                name,
                dir: _,
                skipped,
                done,
                total,
            } => serde_json::json!({
                "event": "artifact",
                "name": name,
                "state": if *skipped { "skipped" } else { "done" },
                "done": done,
                "total": total,
            }),
            Event::Retrying {
                name,
                attempt,
                reason,
            } => serde_json::json!({
                "event": "retrying",
                "name": name,
                "attempt": attempt,
                "reason": reason,
            }),
            Event::Summary(s) => serde_json::json!({
                "event": "summary",
                "uploaded": s.uploaded,
                "downloaded": s.downloaded,
                "skipped": s.skipped,
                "failed": s.failed,
            }),
        }
    }
}

/// Coalescing progress sender: at most one byte event per MIN_INTERVAL per (name, dir).
#[derive(Clone)]
pub struct Progress {
    tx: mpsc::UnboundedSender<Event>,
    last: Arc<Mutex<HashMap<(String, Dir), Instant>>>,
}

const MIN_INTERVAL: Duration = Duration::from_millis(200);

impl Progress {
    pub fn new(tx: mpsc::UnboundedSender<Event>) -> Self {
        Self {
            tx,
            last: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<Event> {
        self.tx.clone()
    }

    pub async fn started(&self, name: &str, dir: Dir, total: Option<u64>) {
        let _ = self.tx.send(Event::ArtifactStarted {
            name: name.to_owned(),
            dir,
            total,
        });
    }

    pub async fn bytes(&self, name: &str, dir: Dir, done: u64, total: Option<u64>) {
        let key = (name.to_owned(), dir);
        let mut last = self.last.lock().await;
        let now = Instant::now();
        let due = match last.get(&key) {
            Some(t) => now - *t >= MIN_INTERVAL,
            None => true,
        };
        if due {
            last.insert(key, now);
            drop(last);
            let _ = self.tx.send(Event::ArtifactBytes {
                name: name.to_owned(),
                dir,
                done,
                total,
            });
        }
    }

    pub async fn done(&self, name: &str, dir: Dir, skipped: bool, done: u64, total: Option<u64>) {
        let _ = self.tx.send(Event::ArtifactDone {
            name: name.to_owned(),
            dir,
            skipped,
            done,
            total,
        });
    }

    pub fn summary(&self, summary: &Summary) {
        let _ = self.tx.send(Event::Summary(summary.clone()));
    }

    /// Plan event from an action list: for down, Upload actions mean
    /// "the local copy is complete" and land in skip.
    pub fn plan(&self, actions: &[Action], dir: Dir) {
        let mut upload = Vec::new();
        let mut download = Vec::new();
        let mut skip = Vec::new();
        for a in actions {
            match a {
                Action::Upload { name, .. } => match dir {
                    Dir::Up => upload.push(name.to_string()),
                    Dir::Down => skip.push(name.to_string()),
                },
                Action::Download { name, .. } => match dir {
                    Dir::Up => skip.push(name.to_string()),
                    Dir::Down => download.push(name.to_string()),
                },
                Action::Skip { name, .. } => skip.push(name.to_string()),
            }
        }
        let _ = self.tx.send(Event::Plan {
            upload,
            download,
            skip,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn coalesces_byte_events() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let p = Progress::new(tx);
        p.bytes("a", Dir::Down, 1, None).await;
        p.bytes("a", Dir::Down, 2, None).await; // immediately — coalesced away
        p.bytes("b", Dir::Down, 1, None).await; // another name — passes
        assert!(matches!(
            rx.recv().await,
            Some(Event::ArtifactBytes { done: 1, .. })
        ));
        assert!(matches!(
            rx.recv().await,
            Some(Event::ArtifactBytes { name, .. }) if name == "b"
        ));
        assert!(rx.try_recv().is_err());
    }
}
