//! Background and pool machinery that outlives a single request: the idle
//! reaper (suspend, archive, destroy tiers) and pre-warmed pools.

pub mod idle_reaper;
pub mod pool;
pub mod pool_claim;
pub mod pool_replenisher;
pub mod shutdown;
