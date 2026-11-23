//! One module per API resource. Each exposes handlers (and, for resources
//! with several routes, a `router()`); `crate::http::build_router` is the only
//! place that assembles them and decides which auth layer guards which.

pub mod drives;
pub mod exec;
pub mod fs;
pub mod images;
pub mod logs;
pub mod metrics;
pub mod mounts;
pub mod pool;
pub mod preview;
pub mod pty;
pub mod sandbox;
pub mod sandbox_name;
pub mod snapshot;
pub mod snapshot_history;
pub mod tunnel;
