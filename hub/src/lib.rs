//! The Hikaya hub: a small server that keeps the complete, encrypted archive
//! and lets a user's devices sync through it. See `docs/adr/0002` in the
//! repository for the design.

pub mod api;
pub mod db;
pub mod devices;
pub mod state;
pub mod sync;
pub mod vault;
