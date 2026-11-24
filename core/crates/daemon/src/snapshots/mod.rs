//! On-disk snapshot state. `held` is the live set of resumable snapshots
//! (each pins a network lease); `retired` is lease-free checkpoints kept for
//! time-travel restore. Separate roots so reconciliation of one never has to
//! understand the other's invariants. `layout` holds the directory layout and
//! durable file primitives both share; `ops` holds the lifecycle operations
//! (take, resume, fork, delete, archive) that act on them.

pub mod held;
pub mod layout;
pub mod ops;
pub mod retired;

#[cfg(test)]
mod test_support;
