use napi::threadsafe_function::ThreadsafeFunction;
use napi::{Error, Result, Status};
use tokio::sync::mpsc;

use crate::error::js_error_message;
use crate::mapping;

/// The parsed JSON event the `onEvent` callback receives.
pub(crate) type EventCallback =
    ThreadsafeFunction<serde_json::Value, (), serde_json::Value, Status, false>;

/// The drain handle of a started event pump.
/// The pump ends with the first callback failure, if one happens.
pub(crate) type Pump = tokio::task::JoinHandle<Result<()>>;
/// Drain the core event stream into the JS callback.
///
/// Every event is awaited through `call_async_catch`: a throw inside the callback ends the pump with that error instead of becoming a global uncaught exception.
/// The pump ends when the facade drops and the channel closes, so the promise settles only after every event has been handed to JS.
pub(crate) fn spawn_pump(
    mut rx: mpsc::UnboundedReceiver<nexus_raw_core::Event>,
    on_event: Option<EventCallback>,
) -> Option<Pump> {
    on_event.map(|tsfn| {
        // tokio::spawn, not the napi re-export: the re-export disappears under the noop feature that unit tests need for linking.
        // Inside a napi async fn the current runtime is napi's own tokio RT, so both calls land on the same workers (napi-3.13 tokio_runtime.rs: `spawn` is `RT.spawn`).
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let Err(callback) = tsfn.call_async_catch(mapping::event_to_json(&event)).await {
                    return Err(Error::new(
                        Status::GenericFailure,
                        format!("the onEvent callback failed: {callback}"),
                    ));
                }
            }
            Ok(())
        })
    })
}

/// Wait for the pump to drain before the promise settles.
///
/// A callback failure rejects the command promise even when the command itself succeeded.
pub(crate) async fn finish_pump(pump: Option<Pump>) -> Result<()> {
    match pump {
        None => Ok(()),
        Some(handle) => match handle.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(callback)) => Err(callback),
            Err(join) => Err(Error::new(
                Status::GenericFailure,
                format!("event pump failed: {join}"),
            )),
        },
    }
}

/// Drain the event pump, then map `error` for JS.
/// The caller must see the events that led to the failure before the promise rejects.
/// The command's exit code and hint stay untouched: a callback failure only adds a note above the machine lines.
pub(crate) async fn finish_pump_err(pump: Option<Pump>, error: nexus_raw_core::Error) -> Error {
    let payload = mapping::error_payload(&error);
    let mut text = error.to_string();
    if let Some(handle) = pump {
        if let Ok(Err(callback)) = handle.await {
            text.push_str("\nonEvent callback also failed: ");
            text.push_str(&callback.reason);
        }
    }
    js_error_message(text, &payload)
}
