//! Cutline's engine: decides every ffmpeg call (pure planner), solves fit to
//! size, estimates, and runs export jobs in the background.

pub mod engine;
pub mod estimate;
pub mod fit;
pub mod job;
pub mod plan;
pub mod request;
pub mod store;

pub use engine::{Engine, EngineEvent};
pub use job::{JobDetails, JobId, JobOutput};
pub use request::ExportRequest;
