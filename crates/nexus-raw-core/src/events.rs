//! Progress event stream with byte coalescing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::mpsc;

#[cfg(not(target_arch = "wasm32"))]
use crate::sync::diff::Action;

/// Transfer direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dir {
    Up,
    Down,
}

impl Dir {
    #[must_use]
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
    /// Names this call deleted (`rm`): transfers never delete, so they stay at 0.
    pub removed: usize,
    pub failed: Vec<String>,
}

/// Events the core emits.
/// The NDJSON shapes are fixed by golden tests.
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
    /// Deletion progress (§5.4): the name's objects are about to be `DELETEd`, marker first.
    Removing {
        name: String,
    },
    /// The name is gone: at least one of its objects was deleted by this call.
    Removed {
        name: String,
    },
    /// The name was already absent: every DELETE of it answered 404.
    Missing {
        name: String,
    },
    Summary(Summary),
}

impl Event {
    /// The event's JSON form.
    /// Wrappers serialize it 1:1.
    ///
    /// ```rust
    /// # use nexus_raw_core::{Event, Summary};
    /// let event = Event::Summary(Summary::default());
    /// let line = event.to_json().to_string();
    /// assert!(line.contains(r#""event":"summary""#));
    /// ```
    #[must_use]
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
            Event::Removing { name } => serde_json::json!({
                "event": "removing",
                "name": name,
            }),
            Event::Removed { name } => serde_json::json!({
                "event": "removed",
                "name": name,
            }),
            Event::Missing { name } => serde_json::json!({
                "event": "missing",
                "name": name,
            }),
            Event::Summary(s) => serde_json::json!({
                "event": "summary",
                "uploaded": s.uploaded,
                "downloaded": s.downloaded,
                "skipped": s.skipped,
                "removed": s.removed,
                "failed": s.failed,
            }),
        }
    }
}

/// Coalescing progress sender: at most one byte event per `MIN_INTERVAL` per (name, dir).
/// The throttle key is the hash of (name, dir): the lookup runs on every chunk, so it
/// must not allocate. A hash collision only merges two throttle slots, which is harmless.
#[derive(Clone)]
pub struct Progress {
    tx: mpsc::UnboundedSender<Event>,
    last: Arc<std::sync::Mutex<HashMap<u64, Instant>>>,
}

const MIN_INTERVAL: Duration = Duration::from_millis(200);

/// The throttle slot of a (name, dir) pair: hash-based, allocation-free on the hot path.
fn throttle_key(name: &str, dir: Dir) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (name, dir).hash(&mut h);
    h.finish()
}

impl Progress {
    #[must_use]
    pub fn new(tx: mpsc::UnboundedSender<Event>) -> Self {
        Self {
            tx,
            last: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    #[must_use]
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
        let key = throttle_key(name, dir);
        let now = Instant::now();
        let due = {
            let last = self.last.lock().expect("progress throttle lock");
            match last.get(&key) {
                Some(t) => now - *t >= MIN_INTERVAL,
                None => true,
            }
        };
        if due {
            self.last
                .lock()
                .expect("progress throttle lock")
                .insert(key, now);
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

    /// Deletion progress: the name's objects are about to be `DELETEd`.
    pub async fn removing(&self, name: &str) {
        let _ = self.tx.send(Event::Removing {
            name: name.to_owned(),
        });
    }

    /// The name is gone: at least one of its objects was deleted by this call.
    pub async fn removed(&self, name: &str) {
        let _ = self.tx.send(Event::Removed {
            name: name.to_owned(),
        });
    }

    /// The name was already absent: every DELETE of it answered 404.
    pub async fn missing(&self, name: &str) {
        let _ = self.tx.send(Event::Missing {
            name: name.to_owned(),
        });
    }

    pub fn summary(&self, summary: &Summary) {
        let _ = self.tx.send(Event::Summary(summary.clone()));
    }

    /// Plan event from an action list: for down, Upload actions mean "the local copy is complete" and land in skip.
    #[cfg(not(target_arch = "wasm32"))]
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

    /// Plan event for a mirror: copies are destination-facing writes and land in `upload`, so the event keeps the transfer schema untouched.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn plan_mirror(&self, actions: &[crate::sync::mirror::MirrorAction]) {
        let mut upload = Vec::new();
        let mut skip = Vec::new();
        for a in actions {
            match a {
                crate::sync::mirror::MirrorAction::Copy { name, .. } => {
                    upload.push(name.to_string());
                }
                crate::sync::mirror::MirrorAction::Skip { name, .. } => {
                    skip.push(name.to_string());
                }
            }
        }
        let _ = self.tx.send(Event::Plan {
            upload,
            download: Vec::new(),
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
        p.bytes("a", Dir::Down, 2, None).await; // coalesced away immediately
        p.bytes("b", Dir::Down, 1, None).await; // another name, so it passes
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
