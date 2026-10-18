//! Captures (spec §5, §6.1): what the box or `fastcapture add` collected, how it is named and
//! written into the notebook as a raw source with its attachments, and the box's draft.

pub mod detect;
pub mod draft;
pub mod file;
pub mod model;
pub mod raw;
pub mod submit;
pub mod ulid;

pub use model::{Attachment, Capture, Kind};
