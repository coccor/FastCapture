//! The processing queue: it writes and lists jobs, which the runner takes to a model.

pub mod job;
pub mod store;

pub use job::{Job, State};
pub use store::Store;
