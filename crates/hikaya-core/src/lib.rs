//! Platform-independent logic for Hikaya.
//!
//! Nothing in this crate touches audio, the UI or the network, so it can be
//! tested on any machine. The desktop app and the hub both depend on it.

pub mod corrections;
pub mod fillers;
pub mod rules;
pub mod token;
