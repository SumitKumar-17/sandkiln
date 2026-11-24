//! Sandbox lifecycle machinery shared by the routes and the background tasks:
//! create (`create`, `create_options`), stop/destroy (`stop`), the idle reaper
//! (suspend, archive, destroy tiers), pre-warmed pools and shutdown.

pub mod create;
pub mod create_options;
pub mod idle_reaper;
pub mod pool;
pub mod pool_claim;
pub mod pool_replenisher;
pub mod shutdown;
pub mod vm_watcher;
pub mod stop;
