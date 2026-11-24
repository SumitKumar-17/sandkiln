//! Notices sandboxes whose Firecracker process has exited on its own (the
//! guest ran `reboot -f`, `kill -TERM 1`, or crashed) and tears them down.
//!
//! Without this the sandbox stayed in `GET /sandboxes` with a dead VM, every
//! call to it failing with a connection error, until someone deleted it (the
//! daemon's own stop paths remove a sandbox from the map before stopping its
//! VM, so a sandbox the watcher sees dead is always one it did not stop).
//! Teardown is the same as a destroy: the tap and address return to the pool,
//! the egress chain and rootfs are removed, the pool slot is released, and the
//! history row ends with `vm_exited`. Nothing is snapshotted: a dead VM has no
//! state to save.

use crate::lifecycle::stop::destroy_with_reason;
use crate::state::AppState;
use std::sync::Arc;
use std::time::Duration;

/// How often to look. A VM check is one non-blocking `waitpid` per sandbox.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

pub async fn run(state: Arc<AppState>) {
    loop {
        tokio::time::sleep(WATCH_INTERVAL).await;
        reap_dead_sandboxes(&state).await;
    }
}

/// Ids of the sandboxes whose VM process has exited. The sandbox lock is held
/// only for the non-blocking checks.
fn find_dead(state: &AppState) -> Vec<String> {
    let mut sandboxes = state.sandboxes.lock().unwrap();
    sandboxes.iter_mut().filter_map(|(id, sandbox)| sandbox.vm.exit_status().map(|_| id.clone())).collect()
}

async fn reap_dead_sandboxes(state: &Arc<AppState>) {
    for id in find_dead(state) {
        tracing::warn!(sandbox_id = %id, "sandbox VM exited on its own (the guest shut down or crashed): cleaning it up");
        // An error means a request deleted it between the check and here: nothing left to do.
        if destroy_with_reason(state.clone(), id.clone(), sandkiln_store::EndReason::VmExited).await.is_ok() {
            state.metrics.record_vm_exit();
        }
    }
}
