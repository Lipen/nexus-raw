//! NDJSON rendering: one serialized event per stdout line, nothing else.

use nexus_raw_core::Event;

pub(crate) fn line(ev: &Event) {
    let value = ev.to_json();
    let text = serde_json::to_string(&value).expect("core event JSON is always serializable");
    println!("{text}");
}
