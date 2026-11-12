//! What every todai frontend shares: open boards (`tab`), the vim-like
//! keyboard state machine (`nav`), card text, the server client, and the
//! operator's config and session. No GUI or terminal code: the window
//! (`src/gui`) and the TUI (`crates/todai-tui`) translate their input events
//! into `nav::Input` and draw from the same state.

pub mod boardlist;
pub mod cardtext;
pub mod config;
pub mod follow;
pub mod ipc;
pub mod manage;
pub mod nav;
pub mod notify;
pub mod remote;
pub mod session;
pub mod tab;
pub mod textnav;
pub mod tracked;

pub use todai_core::store;
