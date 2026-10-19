//! Turning queued captures into notes, in the background.

pub mod backoff;
#[cfg(test)]
mod end_to_end;
pub mod from_settings;
pub mod pages;
pub mod process;
pub mod usage;
pub mod worker;

pub use worker::{Factory, Status, Waker, Worker};
