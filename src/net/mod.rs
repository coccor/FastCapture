//! HTTP: talking to model servers, and fetching the pages of captured links.

pub mod fetch;
pub mod http;
#[cfg(test)]
pub(crate) mod test_server;
