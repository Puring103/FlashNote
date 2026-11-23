//! Desktop compatibility facade; shared logic lives in suojian-core.
pub use suojian_core::*;
pub mod cli;
#[cfg(windows)]
pub mod cli_pipe;

#[cfg(target_os = "linux")]
pub mod linux_keyring;
