//! Cutline's data model. Pure types and operations, no IO.
//!
//! Everything that touches the disk or spawns processes lives in other
//! crates; this one is shared by all of them.

pub mod format;
pub mod profile;
pub mod project;
pub mod time;

pub use profile::*;
pub use project::*;
pub use time::Timestamp;
