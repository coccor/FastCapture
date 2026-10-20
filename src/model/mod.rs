//! Asking a model for a note: the prompt, the images, the request every backend sends, and the
//! backends themselves.

pub mod backend;
pub mod claude;
pub mod claude_code;
pub mod copilot;
pub mod images;
pub mod local;
pub mod openai;
pub mod prompt;
pub mod request;

pub use backend::{Backend, BackendError, Usage};
