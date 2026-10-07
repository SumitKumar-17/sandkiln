//! Pre-warmed snapshot pools: keep a small number of ready-to-resume
//! snapshots around per (image, resource-config), so a matching
//! `POST /sandboxes` can resume one instead of paying full cold-create
//! cost. Full design, the `warm_count`/`max_count` split, and the
//! producer/consumer queueing this implements are written up in
//! `docs/architecture/02-vm-boot-and-latency.md` — this comment only
//! keeps the invariants a reader editing this file needs at hand:
//!
//! - A request with `drives` or a custom `rate_limit` never matches a
//!   pool (both are baked into a VM at boot time; a warm snapshot was
//!   booted with neither) and never queues on one either — falls through
//!   to cold create instead.
//! - A queued claim waits at most `pool_claim::POOL_QUEUE_TIMEOUT`, then
//!   returns a real `503` rather than hanging the caller.
//! - Pool *configuration* lives only in `AppState::pools` — not durable
//!   across a daemon restart the way snapshots themselves are.

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Notify;

/// A configured pool's identity and target size. `vcpu_count`/
/// `mem_size_mib` are resolved concrete values (see
/// `routes_pool::create_pool`), not the request-style `Option<T>`
/// override shape — so matching a request against a pool is plain
/// `PoolKey` equality, no daemon-default threading needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolConfig {
    pub id: String,
    pub image_id: Option<String>,
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
    pub warm_count: u32,
    /// Max live instances (warm + claimed) at once. `None` = unbounded
    /// (original behavior: a claim past warm just cold-creates).
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

/// One configured pool's live state: config, warm snapshot ids (already
/// in `AppState::snapshots`), and the live-instance count. FIFO since
/// replenishment order should roughly match claim order, though nothing
/// depends on it being exact.
pub struct Pool {
    pub config: PoolConfig,
    warm: VecDeque<String>,
    /// Live instances of this profile right now (warm-resumed or
    /// cold-created under `max_count` headroom), for as long as that
    /// sandbox stays live.
    claimed: u32,
    /// Woken when a slot might have freed (`push_warm`/`record_release`),
    /// so a queued claim (`routes_sandbox::create_sandbox_core`) re-checks
    /// instead of polling. `notify_waiters`, not `notify_one`: every
    /// waiter re-evaluates the real condition on wake, so an extra wakeup
    /// is just a harmless re-check — standard `Notify`-as-condvar.
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

    /// `warm_count`, capped by remaining `max_count` headroom if set —
    /// keeps a bounded pool from ever exceeding its own ceiling.
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

    /// Pops one ready warm snapshot id, if any — resuming it is the
    /// caller's job (`routes_sandbox::create_sandbox_core`); this only
    /// manages the queue, kept testable without a real `AppState`.
    pub fn take_warm(&mut self) -> Option<String> {
        self.warm.pop_front()
    }

    /// Whether a new live instance may start counting against this pool —
    /// always `true` when `max_count` is unset.
    pub fn has_room_for_new_claim(&self) -> bool {
        self.config.max_count.is_none_or(|max| self.claimed < max)
    }

    /// Reserves a slot before actually resuming/booting, so a concurrent
    /// claim can't over-commit past `max_count` in the race window before
    /// the `Sandbox` is inserted. Pair with `record_release` on any exit
    /// path that doesn't end in a tracked `Sandbox` (failed resume/boot/
    /// health check) — see `routes_sandbox::PoolClaimGuard`.
    pub fn record_claim(&mut self) {
        self.claimed += 1;
    }

    /// Releases a slot (a reservation that didn't pan out, or a real
    /// pool-sourced sandbox stopping) and wakes anyone queued.
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
