//! Shared boundary for native helpers. This is not an OS filesystem sandbox.
mod environment;
mod storage;
pub(crate) use environment::configure;
pub use storage::protect_private_dir;

#[cfg(test)]
mod tests;
