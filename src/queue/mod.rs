//! The processing queue. M1 writes and lists jobs; the runner that takes them to a
//! model comes with M2.

pub mod job;
pub mod store;

pub use job::{Job, State};
pub use store::Store;
