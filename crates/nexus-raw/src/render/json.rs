//! NDJSON rendering: one serialized event per stdout line, nothing else.

use crate::render::outln;
use nexus_raw_core::Event;

pub(crate) fn line(ev: &Event, stderr: bool) {
    let value = ev.to_json();
    let text = serde_json::to_string(&value).expect("core event JSON is always serializable");
    if stderr {
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), "{text}");
    } else {
        outln!("{text}");
    }
}
