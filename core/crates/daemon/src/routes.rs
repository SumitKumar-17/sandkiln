use crate::error::AppError;
use crate::sandbox::Sandbox;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use sandkiln_protocol::{Request, Response as AgentResponse};
use sandkiln_vmm::network::Lease;
use sandkiln_vmm::vm::{Vm, VmConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub struct CreateSandboxRequest {
    #[serde(default)]
    tags: HashMap<String, String>,
}

#[derive(Serialize)]
pub struct CreateSandboxResponse {
    id: String,
}

#[tracing::instrument(skip(state, body))]
pub async fn create_sandbox(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Json<CreateSandboxResponse>, AppError> {
    let request: CreateSandboxRequest = if body.is_empty() {
        CreateSandboxRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?
    };

    let id = Uuid::new_v4().to_string();
    let rootfs_path = std::env::temp_dir().join(format!("sandkiln-rootfs-{id}.ext4"));

    let (vm, network) = tokio::task::spawn_blocking({
        let state = state.clone();
        let rootfs_path = rootfs_path.clone();
        move || -> std::io::Result<(Vm, Lease)> {
            // Copying the rootfs and leasing a network are independent —
            // running them concurrently overlaps the (currently dominant)
            // cost of the rootfs copy with the lease instead of paying for
            // both serially.
            let (copy_result, lease_result) = std::thread::scope(|scope| {
                let copy_handle = scope.spawn(|| clone_rootfs(&state.config.base_rootfs_path, &rootfs_path));
                let lease_handle = scope.spawn(|| state.network.lease());
                (copy_handle.join().expect("rootfs copy thread panicked"), lease_handle.join().expect("lease thread panicked"))
            });
            copy_result?;
            let lease = lease_result?;

            let vm = Vm::boot(&VmConfig {
                firecracker_bin: state.config.firecracker_bin.clone(),
                kernel_path: state.config.kernel_path.clone(),
                rootfs_path,
                vcpu_count: state.config.vcpu_count,
                mem_size_mib: state.config.mem_size_mib,
                network: Some(lease.config.clone()),
            });
            match vm {
                Ok(vm) => Ok((vm, lease)),
                Err(e) => {
                    let _ = state.network.release(lease);
                    Err(e)
                }
            }
        }
    })
    .await
    .expect("boot task panicked")?;

    let sandbox =
        Sandbox { id: id.clone(), vm, network, rootfs_path, tags: request.tags, created_at: SystemTime::now() };
    state.sandboxes.lock().unwrap().insert(id.clone(), sandbox);

    Ok(Json(CreateSandboxResponse { id }))
}

#[derive(Serialize)]
pub struct SandboxSummary {
    id: String,
    created_at_unix: u64,
    tags: HashMap<String, String>,
}

#[derive(Serialize)]
pub struct ListSandboxesResponse {
    sandboxes: Vec<SandboxSummary>,
}

/// Filters by tag by passing `?tag.<key>=<value>` query params — a
/// sandbox must match every one given, if any are given.
pub async fn list_sandboxes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<ListSandboxesResponse> {
    let tag_filters: Vec<(&str, &str)> = query
        .iter()
        .filter_map(|(k, v)| k.strip_prefix("tag.").map(|key| (key, v.as_str())))
        .collect();

    let sandboxes = state
        .sandboxes
        .lock()
        .unwrap()
        .values()
        .filter(|s| tag_filters.iter().all(|(k, v)| s.tags.get(*k).map(String::as_str) == Some(v)))
        .map(|s| SandboxSummary {
            id: s.id.clone(),
            created_at_unix: s.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            tags: s.tags.clone(),
        })
        .collect();
    Json(ListSandboxesResponse { sandboxes })
}

#[tracing::instrument(skip(state))]
pub async fn stop_sandbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;

    tokio::task::spawn_blocking(move || {
        let _ = sandbox.vm.stop();
        let _ = state.network.release(sandbox.network);
        let _ = std::fs::remove_file(&sandbox.rootfs_path);
    })
    .await
    .expect("stop task panicked");

    Ok(StatusCode::NO_CONTENT)
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

#[tracing::instrument(skip(state, body), fields(command = %body.command))]
pub async fn exec(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ExecRequestBody>,
) -> Result<Json<ExecResponseBody>, AppError> {
    let response = call_agent(state, id, Request::Exec { command: body.command, args: body.args }).await?;

    match response {
        AgentResponse::Exec { stdout, stderr, exit_code } => Ok(Json(ExecResponseBody { stdout, stderr, exit_code })),
        other => Err(AppError::Internal(std::io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

#[derive(Deserialize)]
pub struct ReadFileRequestBody {
    path: String,
}

#[derive(Serialize)]
pub struct ReadFileResponseBody {
    content_base64: String,
}

#[tracing::instrument(skip(state, body), fields(path = %body.path))]
pub async fn read_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ReadFileRequestBody>,
) -> Result<Json<ReadFileResponseBody>, AppError> {
    let response = call_agent(state, id, Request::ReadFile { path: body.path }).await?;
    match response {
        AgentResponse::File { content_base64 } => Ok(Json(ReadFileResponseBody { content_base64 })),
        AgentResponse::Error { message } => Err(AppError::BadRequest(message)),
        other => Err(AppError::Internal(std::io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

#[derive(Deserialize)]
pub struct WriteFileRequestBody {
    path: String,
    content_base64: String,
}

#[tracing::instrument(skip(state, body), fields(path = %body.path))]
pub async fn write_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<WriteFileRequestBody>,
) -> Result<StatusCode, AppError> {
    let response =
        call_agent(state, id, Request::WriteFile { path: body.path, content_base64: body.content_base64 }).await?;
    match response {
        AgentResponse::Ok => Ok(StatusCode::NO_CONTENT),
        AgentResponse::Error { message } => Err(AppError::BadRequest(message)),
        other => Err(AppError::Internal(std::io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

/// Shared by every route that just forwards one request to the guest
/// agent and reports its response — exec/read/write all fit this shape.
async fn call_agent(state: Arc<AppState>, id: String, request: Request) -> Result<AgentResponse, AppError> {
    tokio::task::spawn_blocking(move || {
        let sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        sandbox.vm.call(&request).map_err(AppError::from)
    })
    .await
    .expect("agent call task panicked")
}

/// Clones the base rootfs for one sandbox. Uses `cp --reflink=auto`
/// rather than `std::fs::copy` so this becomes an instant copy-on-write
/// clone for free on a filesystem that supports it (XFS, Btrfs) — on
/// ext4 (what the dev box runs) `--reflink=auto` just falls back to an
/// ordinary copy, so this has no effect there, but costs nothing either.
fn clone_rootfs(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cp").arg("--reflink=auto").arg(src).arg(dst).status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!("cp --reflink=auto {src:?} {dst:?} failed: {status}")));
    }
    Ok(())
}
