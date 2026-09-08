//! Pre-warmed snapshot pools: keep a small number of ready-to-resume
//! snapshots around per (image, resource-config), so a matching
//! `POST /sandboxes` can resume one instead of paying full cold-create
//! cost. See `ROADMAP.md`'s "Persistence and snapshotting" section,
//! "Pre-warmed snapshot pool", for the design this implements a first,
//! deliberately narrower slice of — and its own finding that the actual
//! win is skipping per-create rootfs/network setup, not the boot-vs-resume
//! gap itself (both are already small).
//!
//! Scoped honestly, not silently incomplete:
//! - **No `max_count` ceiling or queueing yet.** A pool only ever tops
//!   itself back up toward `warm_count`; a claim that arrives while
//!   nothing is warm just falls through to a normal cold create, exactly
//!   like before pools existed. The roadmap's ceiling+queueing design is
//!   a real follow-up, not implemented here.
//! - **A request with `drives` or a custom `rate_limit` never matches a
//!   pool.** Both are baked into a VM's state at boot time, and a warm
//!   snapshot was booted with neither — satisfying either from a warm
//!   snapshot isn't possible without teaching the pool to vary on them
//!   too. Falls through to cold create rather than silently ignoring the
//!   request's own drives/rate_limit.
//! - **Pool configuration lives only in memory** (`AppState::pools`), not
//!   durable across a daemon restart the way snapshots themselves are —
//!   a caller that needs a pool to survive a restart has to re-`POST
//!   /pools` afterward. See `crate::pool_replenisher` for the background
//!   task that actually keeps a pool topped up.

use std::collections::VecDeque;

/// A configured pool's identity and target size. `vcpu_count`/
/// `mem_size_mib` are already resolved to concrete values (see
/// `routes_pool::create_pool`) — not the request-style `Option<T>`
/// "override the daemon default" shape used elsewhere — specifically so
/// matching a `POST /sandboxes` request against a pool is plain equality
/// (`PoolKey`) rather than needing the daemon's default config threaded
/// through every comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolConfig {
    pub id: String,
    pub image_id: Option<String>,
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
    pub warm_count: u32,
}

/// What a `POST /sandboxes` request is matched against — see
/// `PoolConfig`'s doc comment for why both sides compare resolved values.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub image_id: Option<String>,
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
}

impl PoolConfig {
    pub fn key(&self) -> PoolKey {
        PoolKey { image_id: self.image_id.clone(), vcpu_count: self.vcpu_count, mem_size_mib: self.mem_size_mib }
    }
}

/// One configured pool's live state: its config plus the snapshot ids
/// (already present in `AppState::snapshots`) currently sitting warm,
/// ready to be resumed. FIFO purely so replenishment order roughly
/// matches claim order — nothing depends on that ordering being exact.
pub struct Pool {
    pub config: PoolConfig,
    warm: VecDeque<String>,
}

impl Pool {
    pub fn new(config: PoolConfig) -> Self {
        Self { config, warm: VecDeque::new() }
    }

    pub fn warm_ready(&self) -> usize {
        self.warm.len()
    }

    pub fn needs_replenish(&self) -> bool {
        (self.warm.len() as u32) < self.config.warm_count
    }

    pub fn push_warm(&mut self, snapshot_id: String) {
        self.warm.push_back(snapshot_id);
    }

    /// Takes one warm snapshot id for a matching claim, if any is ready.
    /// The caller still has to actually resume it (see
    /// `routes_sandbox::create_sandbox_core`) — this only ever manages
    /// the queue itself, so it stays testable without a real `AppState`.
    pub fn take_warm(&mut self) -> Option<String> {
        self.warm.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PoolConfig {
        PoolConfig { id: "p1".to_string(), image_id: None, vcpu_count: 2, mem_size_mib: 512, warm_count: 2 }
    }

    #[test]
    fn a_fresh_pool_has_nothing_warm_and_needs_replenishing() {
        let pool = Pool::new(config());
        assert_eq!(pool.warm_ready(), 0);
        assert!(pool.needs_replenish());
        assert!(pool.config.key() == PoolKey { image_id: None, vcpu_count: 2, mem_size_mib: 512 });
    }

    #[test]
    fn pushing_warm_snapshots_up_to_the_target_stops_needing_replenishment() {
        let mut pool = Pool::new(config());
        pool.push_warm("snap-1".to_string());
        assert!(pool.needs_replenish());
        pool.push_warm("snap-2".to_string());
        assert_eq!(pool.warm_ready(), 2);
        assert!(!pool.needs_replenish());
    }

    #[test]
    fn take_warm_pops_in_fifo_order_and_needs_replenishment_again() {
        let mut pool = Pool::new(config());
        pool.push_warm("snap-1".to_string());
        pool.push_warm("snap-2".to_string());
        assert_eq!(pool.take_warm(), Some("snap-1".to_string()));
        assert!(pool.needs_replenish());
        assert_eq!(pool.take_warm(), Some("snap-2".to_string()));
        assert_eq!(pool.take_warm(), None);
    }

    #[test]
    fn distinct_image_ids_produce_distinct_keys() {
        let a = PoolConfig { id: "a".to_string(), image_id: Some("img1".to_string()), vcpu_count: 1, mem_size_mib: 128, warm_count: 1 };
        let b = PoolConfig { id: "b".to_string(), image_id: Some("img2".to_string()), vcpu_count: 1, mem_size_mib: 128, warm_count: 1 };
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn distinct_resource_configs_produce_distinct_keys_even_with_the_same_image() {
        let a = PoolConfig { id: "a".to_string(), image_id: None, vcpu_count: 1, mem_size_mib: 128, warm_count: 1 };
        let b = PoolConfig { id: "b".to_string(), image_id: None, vcpu_count: 2, mem_size_mib: 128, warm_count: 1 };
        assert_ne!(a.key(), b.key());
    }
}
