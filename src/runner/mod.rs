//! Turning queued captures into notes, in the background.

pub mod backoff;
pub mod from_settings;
pub mod process;
pub mod worker;

pub use worker::{Factory, Status, Waker, Worker};
