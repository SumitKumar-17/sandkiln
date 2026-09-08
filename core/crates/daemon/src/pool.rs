//! Pre-warmed snapshot pools: keep a small number of ready-to-resume
//! snapshots around per (image, resource-config), so a matching
//! `POST /sandboxes` can resume one instead of paying full cold-create
//! cost. See `ROADMAP.md`'s "Persistence and snapshotting" section,
//! "Pre-warmed snapshot pool", for the design this implements.
//!
//! Two independently optional knobs, since they solve different
//! problems: `warm_count` (latency — how many ready-to-resume snapshots
//! to keep sitting around) and `max_count` (concurrency — the total
//! number of *live* instances this pool's profile may ever have at once,
//! `None` meaning unbounded, today's original behavior). A request
//! matching a pool with room claims a warm snapshot if one's ready, or
//! cold-creates if under `max_count`, or **queues** (via `Pool::notify`)
//! until either happens — see `routes_sandbox::create_sandbox_core`.
//! Every live instance counted against `max_count`, warm or cold-boosted
//! alike, is tracked by `Sandbox::source_pool_id` for exactly as long as
//! it stays that one live sandbox; stopping it (destroy *or* snapshot)
//! releases the slot back (`routes_sandbox::destroy_sandbox_by_id`,
//! `routes_snapshot::snapshot_and_stop`) — a later resume/fork of the
//! resulting snapshot is a fresh creation event, evaluated against
//! whatever pool it matches (if any) at that time, not tied back to this
//! one forever.
//!
//! Scoped honestly, not silently incomplete:
//! - **A request with `drives` or a custom `rate_limit` never matches a
//!   pool.** Both are baked into a VM's state at boot time, and a warm
//!   snapshot was booted with neither — satisfying either from a warm
//!   snapshot isn't possible without teaching the pool to vary on them
//!   too. Falls through to cold create rather than silently ignoring the
//!   request's own drives/rate_limit — and a `drives`/`rate_limit`
//!   request also never queues, even if a `max_count`-bounded pool with
//!   the same image/resources is at capacity, for the same reason: it
//!   was never going to match that pool anyway.
//! - **A queued claim waits a bounded time, not forever** — see
//!   `routes_sandbox::POOL_QUEUE_TIMEOUT` — and returns a real error
//!   (`503`) if nothing frees up in time, rather than hanging the
//!   caller's request indefinitely.
//! - **Pool configuration lives only in memory** (`AppState::pools`), not
//!   durable across a daemon restart the way snapshots themselves are —
//!   a caller that needs a pool to survive a restart has to re-`POST
//!   /pools` afterward. See `crate::pool_replenisher` for the background
//!   task that actually keeps a pool topped up.

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Notify;

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
    /// Maximum number of live instances (warm + claimed, combined) this
    /// pool's profile may ever have at once. `None` is unbounded — the
    /// original behavior, before this field existed: a claim past what's
    /// warm always just cold-creates.
    pub max_count: Option<u32>,
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

/// One configured pool's live state: its config, the snapshot ids
/// (already present in `AppState::snapshots`) currently sitting warm, and
/// how many live instances of this pool's profile currently exist.
/// FIFO purely so replenishment order roughly matches claim order —
/// nothing depends on that ordering being exact.
pub struct Pool {
    pub config: PoolConfig,
    warm: VecDeque<String>,
    /// Live instances of this pool's profile right now — both a resumed
    /// warm claim and a cold-created one under `max_count` headroom count
    /// here, for as long as that one sandbox stays live. See this
    /// module's own doc comment for exactly when this is incremented and
    /// released.
    claimed: u32,
    /// Woken whenever a slot might have freed up — a new warm snapshot
    /// arrives (`push_warm`) or a live claim ends (`record_release`) —
    /// so a queued `POST /sandboxes` waiting on this pool (see
    /// `routes_sandbox::create_sandbox_core`) knows to re-check rather
    /// than poll. `notify_waiters` (not `notify_one`): every waiter
    /// re-evaluates the real condition itself on wake, so waking more
    /// than the one that can actually proceed is harmless, just a wasted
    /// re-check — the standard tokio `Notify`-as-condvar pattern.
    pub notify: Arc<Notify>,
}

impl Pool {
    pub fn new(config: PoolConfig) -> Self {
        Self { config, warm: VecDeque::new(), claimed: 0, notify: Arc::new(Notify::new()) }
    }

    pub fn warm_ready(&self) -> usize {
        self.warm.len()
    }

    pub fn claimed_count(&self) -> u32 {
        self.claimed
    }

    /// How many warm snapshots this pool should actually try to keep
    /// ready right now — `warm_count`, unless `max_count` is set and
    /// already-claimed instances leave less headroom than that, in which
    /// case replenishing only fills the remaining room. Keeps a
    /// `max_count`-bounded pool from ever producing more total instances
    /// (warm + claimed) than its own ceiling allows.
    fn effective_warm_target(&self) -> u32 {
        match self.config.max_count {
            Some(max) => self.config.warm_count.min(max.saturating_sub(self.claimed)),
            None => self.config.warm_count,
        }
    }

    pub fn needs_replenish(&self) -> bool {
        (self.warm.len() as u32) < self.effective_warm_target()
    }

    pub fn push_warm(&mut self, snapshot_id: String) {
        self.warm.push_back(snapshot_id);
        self.notify.notify_waiters();
    }

    /// Takes one warm snapshot id for a matching claim, if any is ready.
    /// The caller still has to actually resume it (see
    /// `routes_sandbox::create_sandbox_core`) — this only ever manages
    /// the queue itself, so it stays testable without a real `AppState`.
    pub fn take_warm(&mut self) -> Option<String> {
        self.warm.pop_front()
    }

    /// Whether a brand-new live instance (resumed from a warm snapshot,
    /// or cold-created) is allowed to start counting against this pool
    /// right now — always `true` when `max_count` is unset.
    pub fn has_room_for_new_claim(&self) -> bool {
        self.config.max_count.is_none_or(|max| self.claimed < max)
    }

    /// Reserves a slot for a new live instance — call before actually
    /// resuming/booting it, so a concurrent second claim can't
    /// over-commit past `max_count` in the race window before the first
    /// one's `Sandbox` is inserted. Pair with `record_release` on any
    /// exit path that doesn't end in a real, tracked `Sandbox` (a failed
    /// resume, a failed cold boot, a failed health check) — see
    /// `routes_sandbox::PoolClaimGuard`.
    pub fn record_claim(&mut self) {
        self.claimed += 1;
    }

    /// Releases a slot — either because the reservation above didn't pan
    /// out, or because a real, live pool-sourced sandbox was just
    /// stopped (destroyed or snapshotted). Wakes anyone queued on this
    /// pool, since this may be exactly the room they were waiting for.
    pub fn record_release(&mut self) {
        self.claimed = self.claimed.saturating_sub(1);
        self.notify.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PoolConfig {
        PoolConfig { id: "p1".to_string(), image_id: None, vcpu_count: 2, mem_size_mib: 512, warm_count: 2, max_count: None }
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
        let a =
            PoolConfig { id: "a".to_string(), image_id: Some("img1".to_string()), vcpu_count: 1, mem_size_mib: 128, warm_count: 1, max_count: None };
        let b =
            PoolConfig { id: "b".to_string(), image_id: Some("img2".to_string()), vcpu_count: 1, mem_size_mib: 128, warm_count: 1, max_count: None };
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn distinct_resource_configs_produce_distinct_keys_even_with_the_same_image() {
        let a = PoolConfig { id: "a".to_string(), image_id: None, vcpu_count: 1, mem_size_mib: 128, warm_count: 1, max_count: None };
        let b = PoolConfig { id: "b".to_string(), image_id: None, vcpu_count: 2, mem_size_mib: 128, warm_count: 1, max_count: None };
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn with_no_max_count_there_is_always_room_and_replenishment_always_targets_warm_count() {
        let mut pool = Pool::new(config());
        pool.record_claim();
        pool.record_claim();
        pool.record_claim();
        assert!(pool.has_room_for_new_claim());
        assert!(pool.needs_replenish());
    }

    #[test]
    fn max_count_blocks_a_new_claim_once_reached() {
        let mut config = config();
        config.max_count = Some(2);
        let mut pool = Pool::new(config);
        assert!(pool.has_room_for_new_claim());
        pool.record_claim();
        assert!(pool.has_room_for_new_claim());
        pool.record_claim();
        assert!(!pool.has_room_for_new_claim());
    }

    #[test]
    fn record_release_frees_a_slot_back_up() {
        let mut config = config();
        config.max_count = Some(1);
        let mut pool = Pool::new(config);
        pool.record_claim();
        assert!(!pool.has_room_for_new_claim());
        pool.record_release();
        assert!(pool.has_room_for_new_claim());
    }

    #[test]
    fn record_release_below_zero_saturates_instead_of_underflowing() {
        let mut pool = Pool::new(config());
        pool.record_release();
        assert_eq!(pool.claimed_count(), 0);
    }

    #[test]
    fn effective_warm_target_is_capped_by_remaining_max_count_headroom() {
        let mut config = config(); // warm_count: 2
        config.max_count = Some(3);
        let mut pool = Pool::new(config);
        // No claims yet: 3 - 0 = 3 headroom, but warm_count (2) is the tighter cap.
        assert!(pool.needs_replenish());
        pool.push_warm("snap-1".to_string());
        pool.push_warm("snap-2".to_string());
        assert!(!pool.needs_replenish());

        // One live claim now: only 3 - 1 = 2 headroom total, and 2 are
        // already warm -- no more room to replenish further even though
        // warm_count itself would otherwise allow one more.
        pool.record_claim();
        assert!(!pool.needs_replenish());
    }

    #[test]
    fn effective_warm_target_never_underflows_when_claimed_exceeds_max_count() {
        let mut config = config();
        config.max_count = Some(1);
        let mut pool = Pool::new(config);
        pool.record_claim();
        pool.record_claim(); // over capacity, e.g. from a race -- must not panic
        assert!(!pool.needs_replenish());
    }
}
