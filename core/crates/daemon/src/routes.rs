use crate::error::AppError;
use crate::sandbox::Sandbox;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use sandkiln_protocol::{Request, Response as AgentResponse};
use sandkiln_vmm::vm::{Vm, VmConfig};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Serialize)]
pub struct CreateSandboxResponse {
    id: String,
}

pub async fn create_sandbox(State(state): State<Arc<AppState>>) -> Result<Json<CreateSandboxResponse>, AppError> {
    let id = Uuid::new_v4().to_string();
    let rootfs_path = std::env::temp_dir().join(format!("sandkiln-rootfs-{id}.ext4"));

    let vm = tokio::task::spawn_blocking({
        let state = state.clone();
        let rootfs_path = rootfs_path.clone();
        move || -> std::io::Result<Vm> {
            std::fs::copy(&state.config.base_rootfs_path, &rootfs_path)?;
            Vm::boot(&VmConfig {
                firecracker_bin: state.config.firecracker_bin.clone(),
                kernel_path: state.config.kernel_path.clone(),
                rootfs_path,
                vcpu_count: state.config.vcpu_count,
                mem_size_mib: state.config.mem_size_mib,
                network: None,
            })
        }
    })
    .await
    .expect("boot task panicked")?;

    let sandbox = Sandbox { id: id.clone(), vm, rootfs_path, created_at: SystemTime::now() };
    state.sandboxes.lock().unwrap().insert(id.clone(), sandbox);

    Ok(Json(CreateSandboxResponse { id }))
}

#[derive(Serialize)]
pub struct SandboxSummary {
    id: String,
    created_at_unix: u64,
}

#[derive(Serialize)]
pub struct ListSandboxesResponse {
    sandboxes: Vec<SandboxSummary>,
}

pub async fn list_sandboxes(State(state): State<Arc<AppState>>) -> Json<ListSandboxesResponse> {
    let sandboxes = state
        .sandboxes
        .lock()
        .unwrap()
        .values()
        .map(|s| SandboxSummary {
            id: s.id.clone(),
            created_at_unix: s.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
        })
        .collect();
    Json(ListSandboxesResponse { sandboxes })
}

pub async fn stop_sandbox(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), AppError> {
    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;

    tokio::task::spawn_blocking(move || {
        let _ = sandbox.vm.stop();
        let _ = std::fs::remove_file(&sandbox.rootfs_path);
    })
    .await
    .expect("stop task panicked");

    Ok(())
}

#[derive(Deserialize)]
pub struct ExecRequestBody {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Serialize)]
pub struct ExecResponseBody {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

pub async fn exec(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ExecRequestBody>,
) -> Result<Json<ExecResponseBody>, AppError> {
    let response = tokio::task::spawn_blocking(move || -> Result<AgentResponse, AppError> {
        let sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        sandbox
            .vm
            .call(&Request::Exec { command: body.command, args: body.args })
            .map_err(AppError::from)
    })
    .await
    .expect("exec task panicked")?;

    match response {
        AgentResponse::Exec { stdout, stderr, exit_code } => Ok(Json(ExecResponseBody { stdout, stderr, exit_code })),
        other => Err(AppError::Internal(std::io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}
