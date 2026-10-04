use napi::{Error, Status};

use crate::mapping;

// ---- helpers ---------------------------------------------------------------

/// Map a core error onto the rejection message: the CLI-style text with the exit line and the hint appended.
/// `index.js` promotes them to `.exitCode` and `.hint` on the rejected Error.
pub(crate) fn js_error(e: nexus_raw_core::Error) -> Error {
    let payload = mapping::error_payload(&e);
    js_error_message(e.to_string(), &payload)
}

/// Build the rejection from a text plus the trailing machine lines of a payload.
pub(crate) fn js_error_message(text: String, payload: &mapping::ErrorPayload) -> Error {
    let mut message = text;
    message.push_str(&format!("\nnxr:exit {}", payload.exit_code));
    if let Some(hint) = &payload.hint {
        message.push_str("\nhint: ");
        message.push_str(hint);
    }
    Error::new(Status::GenericFailure, message)
}
