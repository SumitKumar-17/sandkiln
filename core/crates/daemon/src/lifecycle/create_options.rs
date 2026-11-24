//! Validation of the optional create-time knobs: per-request vCPU/memory
//! overrides, the I/O rate limiter, the egress policy and duplicate drive ids.
//!
//! Pure functions from request DTOs to the values the VMM understands, each
//! failing with a message naming the offending field so the route can return
//! one clear `400` before any resource is allocated.

use crate::http::routes::sandbox::{EgressModeRequest, EgressPolicyRequest, RateLimitRequest};
use sandkiln_vmm::vm::{RateLimiter, TokenBucket};
use std::collections::HashSet;

/// Resolves a per-request override against the configured default/
/// ceiling. `None` → `default`. `0` or above `max` is rejected (`400`),
/// not clamped — a bad request fails loudly instead of silently running
/// with less than expected. A negative value can't reach here at all:
/// these fields deserialize as unsigned, so `serde_json` rejects it first.
pub(crate) fn resolve_resource_override<T>(requested: Option<T>, default: T, max: T, field: &str) -> Result<T, String>
where
    T: PartialOrd + Copy + Default + std::fmt::Display,
{
    match requested {
        None => Ok(default),
        Some(value) if value == T::default() => Err(format!("{field} must be greater than 0")),
        Some(value) if value > max => Err(format!("{field} {value} exceeds the configured maximum of {max}")),
        Some(value) => Ok(value),
    }
}

/// Resolves a requested `rate_limit` into the `RateLimiter` Firecracker
/// understands. `None` → unlimited. Neither sub-field set, or either
/// `0`, is rejected (meaningless, not treated as unlimited) — same
/// convention as `resolve_resource_override`. Each bucket refills to
/// `size` once per second (`refill_time: 1000`), no initial burst — the
/// simplest bytes/ops-per-second mapping; burst tuning isn't exposed yet.
pub(crate) fn resolve_rate_limit(requested: &Option<RateLimitRequest>) -> Result<Option<RateLimiter>, String> {
    let Some(req) = requested else { return Ok(None) };
    if req.bandwidth_bytes_per_sec.is_none() && req.ops_per_sec.is_none() {
        return Err("rate_limit must set at least one of bandwidth_bytes_per_sec/ops_per_sec".to_string());
    }
    let to_bucket = |field: &str, value: Option<u64>| -> Result<Option<TokenBucket>, String> {
        match value {
            None => Ok(None),
            Some(0) => Err(format!("rate_limit.{field} must be greater than 0")),
            Some(size) => Ok(Some(TokenBucket { size, one_time_burst: None, refill_time: 1000 })),
        }
    };
    Ok(Some(RateLimiter {
        bandwidth: to_bucket("bandwidth_bytes_per_sec", req.bandwidth_bytes_per_sec)?,
        ops: to_bucket("ops_per_sec", req.ops_per_sec)?,
    }))
}

/// Validates every CIDR up front — one clear `400` naming the bad entry,
/// rather than a cryptic iptables failure later inside a boot task.
pub(crate) fn resolve_egress_policy(requested: &Option<EgressPolicyRequest>) -> Result<Option<sandkiln_vmm::egress::EgressPolicy>, String> {
    let Some(req) = requested else { return Ok(None) };
    for cidr in req.allow_cidrs.iter().chain(&req.deny_cidrs) {
        sandkiln_vmm::egress::validate_cidr(cidr)?;
    }
    let mode = match req.mode {
        EgressModeRequest::AllowAll => sandkiln_vmm::egress::EgressMode::AllowAll,
        EgressModeRequest::DenyAll => sandkiln_vmm::egress::EgressMode::DenyAll,
    };
    Ok(Some(sandkiln_vmm::egress::EgressPolicy { mode, allow_cidrs: req.allow_cidrs.clone(), deny_cidrs: req.deny_cidrs.clone() }))
}

/// First item already seen, if any.
pub(crate) fn first_duplicate<'a>(mut items: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen = HashSet::new();
    items.find(|item| !seen.insert(*item))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_resource_override_uses_the_default_when_omitted() {
        assert_eq!(resolve_resource_override(None, 2u8, 16u8, "vcpu_count"), Ok(2));
    }

    #[test]
    fn resolve_resource_override_accepts_a_value_within_the_ceiling() {
        assert_eq!(resolve_resource_override(Some(8u8), 2u8, 16u8, "vcpu_count"), Ok(8));
    }

    #[test]
    fn resolve_resource_override_accepts_a_value_exactly_at_the_ceiling() {
        assert_eq!(resolve_resource_override(Some(16u8), 2u8, 16u8, "vcpu_count"), Ok(16));
    }

    #[test]
    fn resolve_resource_override_rejects_zero() {
        assert_eq!(resolve_resource_override(Some(0u8), 2u8, 16u8, "vcpu_count"), Err("vcpu_count must be greater than 0".to_string()));
    }

    #[test]
    fn resolve_resource_override_rejects_above_the_ceiling() {
        assert_eq!(
            resolve_resource_override(Some(17u8), 2u8, 16u8, "vcpu_count"),
            Err("vcpu_count 17 exceeds the configured maximum of 16".to_string())
        );
    }

    #[test]
    fn resolve_resource_override_works_for_mem_size_mib_too() {
        assert_eq!(resolve_resource_override(Some(4096u32), 512u32, 16384u32, "mem_size_mib"), Ok(4096));
        assert_eq!(
            resolve_resource_override(Some(u32::MAX), 512u32, 16384u32, "mem_size_mib"),
            Err(format!("mem_size_mib {} exceeds the configured maximum of 16384", u32::MAX))
        );
    }

    #[test]
    fn first_duplicate_finds_a_repeat() {
        assert_eq!(first_duplicate(["a", "b", "a"].into_iter()), Some("a"));
        assert_eq!(first_duplicate(["a", "b", "c", "b"].into_iter()), Some("b"));
    }

    #[test]
    fn first_duplicate_none_when_all_unique() {
        assert_eq!(first_duplicate(["a", "b", "c"].into_iter()), None);
        assert_eq!(first_duplicate(std::iter::empty()), None);
    }

    #[test]
    fn resolve_egress_policy_returns_none_when_omitted() {
        assert_eq!(resolve_egress_policy(&None), Ok(None));
    }

    #[test]
    fn resolve_egress_policy_accepts_well_formed_cidrs_in_both_lists() {
        let requested = Some(EgressPolicyRequest {
            mode: EgressModeRequest::DenyAll,
            allow_cidrs: vec!["10.0.0.0/8".to_string()],
            deny_cidrs: vec!["192.168.1.1/32".to_string()],
        });
        let policy = resolve_egress_policy(&requested).unwrap().unwrap();
        assert_eq!(policy.mode, sandkiln_vmm::egress::EgressMode::DenyAll);
        assert_eq!(policy.allow_cidrs, vec!["10.0.0.0/8".to_string()]);
        assert_eq!(policy.deny_cidrs, vec!["192.168.1.1/32".to_string()]);
    }

    #[test]
    fn resolve_egress_policy_rejects_a_malformed_allow_cidr() {
        let requested = Some(EgressPolicyRequest {
            mode: EgressModeRequest::AllowAll,
            allow_cidrs: vec!["not-a-cidr".to_string()],
            deny_cidrs: vec![],
        });
        let err = resolve_egress_policy(&requested).unwrap_err();
        assert!(err.contains("not-a-cidr"), "message was: {err}");
    }

    #[test]
    fn resolve_egress_policy_rejects_a_malformed_deny_cidr() {
        let requested = Some(EgressPolicyRequest {
            mode: EgressModeRequest::AllowAll,
            allow_cidrs: vec![],
            deny_cidrs: vec!["10.0.0.0/33".to_string()],
        });
        let err = resolve_egress_policy(&requested).unwrap_err();
        assert!(err.contains("10.0.0.0/33"), "message was: {err}");
    }
}
