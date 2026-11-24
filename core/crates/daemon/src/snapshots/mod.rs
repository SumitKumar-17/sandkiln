//! On-disk snapshot state. `held` is the live set of resumable snapshots
//! (each pins a network lease); `retired` is lease-free checkpoints kept for
//! time-travel restore. Separate roots so reconciliation of one never has to
//! understand the other's invariants.

pub mod held;
pub mod ops;
pub mod retired;
