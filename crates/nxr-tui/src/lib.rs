//! `nxr-tui`: a terminal browser over Nexus raw repositories.
//!
//! One tab per server, one screen per step: the repositories of the server,
//! then the tree of the selected repository at full depth, with subtree
//! downloads, a live filter and a servers overlay that can add a server and
//! save it as a preset.
//!
//! ```text
//! nxr-tui http://127.0.0.1:8081/            one server from the command line
//! nxr-tui                                   every preset of the config file
//! nxr-tui --server main --server backup     two named presets as two tabs
//! ```
//!
//! The library layout: [`args`] parses the command line, [`config`] loads and
//! saves the config file, [`net`] drives the [`nexus_raw_core::Nxr`] facade,
//! [`app`] holds the state machine, [`ui`] renders it, [`tui`] runs the
//! terminal loop and [`smoke`] runs the same flow headless.
//!
//! The bootstrap validates every server before the terminal switches to the
//! alternate screen, so the TUI only opens on known ground.
//!
//! # Example
//!
//! Build the state machine over a fake bootstrap and feed it a message:
//!
//! ```rust
//! use std::path::PathBuf;
//! use std::sync::Arc;
//!
//! use clap::Parser as _;
//! use nxr_tui::args::Args;
//! use nxr_tui::config::ConfigFile;
//! use nxr_tui::{app::App, net};
//!
//! // A runtime context, because the app spawns loads onto tokio.
//! let rt = tokio::runtime::Builder::new_current_thread()
//!     .build()
//!     .expect("runtime");
//! let _guard = rt.enter();
//!
//! let args = Arc::new(Args::parse_from(["nxr-tui", "http://127.0.0.1:8081/"]));
//! let servers = args.resolve_servers(&ConfigFile::default()).unwrap();
//! let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
//! let mut app = App::new(
//!     servers,
//!     Vec::new(),
//!     ConfigFile::default(),
//!     Some(PathBuf::from("/tmp/nxr-tui.toml")),
//!     false,
//!     net::dir_url("http://127.0.0.1:8081").into(),
//!     None,
//!     tx,
//! );
//! app.boot();
//! assert!(!app.quit);
//! ```

pub mod app;
pub mod args;
pub mod config;
pub mod net;
pub mod smoke;
pub mod tui;
pub mod ui;

// The flat public surface: binaries and wrappers import from the crate root.
pub use crate::app::{App, DlEv, DlOutcome, Download, Mode, Msg, Screen, Tab};
pub use crate::args::Args;
pub use crate::config::{ConfigFile, ServerCfg};
