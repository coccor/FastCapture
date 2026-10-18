//! Talking to the running tray app (spec §5.1): `fastcapture add` sends its capture over a
//! named pipe and gets back where it went.

pub mod pipe;
pub mod protocol;
mod security;

pub use protocol::{Reply, Request};
