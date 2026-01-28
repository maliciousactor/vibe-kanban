use anyhow;
use axum::{
    Extension, Router,
    extract::{
        Path, Query, State,
        ws::{WebSocket, WebSocketUpgrade},
    },
    middleware::from_fn_with_state,
    response::{IntoResponse, Json as ResponseJson},
    routing::{get, post},
};
use db::models::{
    execution_process::{ExecutionProcess, ExecutionProcessError, ExecutionProcessStatus},
    execution_process_repo_state::ExecutionProcessRepoState,
};
use deployment::Deployment;
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use nix::errno::Errno;
use serde::Deserialize;
use services::services::container::ContainerService;
use std::{io, error::Error};
use utils::{log_msg::LogMsg, response::ApiResponse};
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError, middleware::load_execution_process_middleware};

/// Check if a WebSocket error is an expected client disconnection
fn is_expected_disconnection(error: &axum::Error) -> bool {
    // Check if it's an I/O error with broken pipe or connection reset
    if let Some(io_error) = error.source().and_then(|s| s.downcast_ref::<io::Error>()) {
        matches!(
            io_error.kind(),
            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
        )
    } else {
        // Walk the error chain to find Errno::EPIPE or Errno::ECONNRESET
        let mut current = error.source();
        while let Some(err) = current {
            // Check for nix::errno::Errno
            if let Some(nix_err) = err.downcast_ref::<Errno>() {
                return *nix_err == Errno::EPIPE || *nix_err == Errno::ECONNRESET;
            }
            current = err.source();
        }
        false
    }
}

#[derive(Debug, Deserialize)]
pub struct SessionExecutionProcessQuery {
    pub session_id: Uuid,
    /// If true, include soft-deleted (dropped) processes in results/stream
    #[serde(default)]
    pub show_soft_deleted: Option<bool>,
}

pub async fn get_execution_process_by_id(
    Extension(execution_process): Extension<ExecutionProcess>,
    State(_deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<ExecutionProcess>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(execution_process)))
}

pub async fn stream_raw_logs_ws(
    ws: WebSocketUpgrade,
    State(deployment): State<DeploymentImpl>,
    Path(exec_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    // Check if the stream exists before upgrading the WebSocket
    let _stream = deployment
        .container()
        .stream_raw_logs(&exec_id)
        .await
        .ok_or_else(|| {
            ApiError::ExecutionProcess(ExecutionProcessError::ExecutionProcessNotFound)
        })?;

    Ok(ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_raw_logs_ws(socket, deployment, exec_id).await {
            tracing::warn!("raw logs WS closed: {}", e);
        }
    }))
}

async fn handle_raw_logs_ws(
    socket: WebSocket,
    deployment: DeploymentImpl,
    exec_id: Uuid,
) -> anyhow::Result<()> {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use executors::logs::utils::patch::ConversationPatch;
    use utils::log_msg::LogMsg;

    // Get the raw stream and convert to JSON patches on-the-fly
    let raw_stream = deployment
        .container()
        .stream_raw_logs(&exec_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("Execution process not found"))?;

    let counter = Arc::new(AtomicUsize::new(0));
    let mut stream = raw_stream.map_ok({
        let counter = counter.clone();
        move |m| match m {
            LogMsg::Stdout(content) => {
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let patch = ConversationPatch::add_stdout(index, content);
                LogMsg::JsonPatch(patch).to_ws_message_unchecked()
            }
            LogMsg::Stderr(content) => {
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let patch = ConversationPatch::add_stderr(index, content);
                LogMsg::JsonPatch(patch).to_ws_message_unchecked()
            }
            LogMsg::Finished => LogMsg::Finished.to_ws_message_unchecked(),
            _ => unreachable!("Raw stream should only have Stdout/Stderr/Finished"),
        }
    });

    // Split socket into sender and receiver
    let (mut sender, mut receiver) = socket.split();

    // Drain (and ignore) any client->server messages so pings/pongs work
    tokio::spawn(async move { while let Some(Ok(_)) = receiver.next().await {} });

    // Forward server messages with proper disconnection handling
    while let Some(item) = stream.next().await {
        match item {
            Ok(msg) => {
                match sender.send(msg).await {
                    Ok(_) => {
                        // Message sent successfully
                    }
                    Err(e) => {
                        // Check if this is an expected disconnection error
                        let is_expected_disconnection = is_expected_disconnection(&e);
                        
                        if is_expected_disconnection {
                            tracing::debug!("Client disconnected (expected disconnection)");
                        } else {
                            tracing::error!("WebSocket send error: {}", e);
                        }
                        
                        // Stop sending messages regardless of error type
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::error!("stream error: {}", e);
                break;
            }
        }
    }
    Ok(())
}

pub async fn stream_normalized_logs_ws(
    ws: WebSocketUpgrade,
    State(deployment): State<DeploymentImpl>,
    Path(exec_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    tracing::debug!(%exec_id, "stream_normalized_logs_ws: entering");
    let stream = deployment
        .container()
        .stream_normalized_logs(&exec_id)
        .await;
    
    tracing::debug!(%exec_id, "stream_normalized_logs_ws: got stream option: {:?}", stream.is_some());
    
    let stream = stream.ok_or_else(|| {
        tracing::warn!(%exec_id, "stream_normalized_logs_ws: stream not found");
        ApiError::ExecutionProcess(ExecutionProcessError::ExecutionProcessNotFound)
    })?;

    // Convert the error type to anyhow::Error and turn TryStream -> Stream<Result<_, _>>
    let stream = stream.err_into::<anyhow::Error>().into_stream();

    tracing::debug!(%exec_id, "stream_normalized_logs_ws: about to call on_upgrade");
    
    Ok(ws.on_upgrade(move |socket| async move {
        tracing::debug!(%exec_id, "stream_normalized_logs_ws: on_upgrade callback started");
        if let Err(e) = handle_normalized_logs_ws(socket, stream).await {
            tracing::warn!("normalized logs WS closed: {}", e);
        }
        tracing::debug!(%exec_id, "stream_normalized_logs_ws: on_upgrade callback ended");
    }))
}

async fn handle_normalized_logs_ws(
    socket: WebSocket,
    stream: impl futures_util::Stream<Item = anyhow::Result<LogMsg>> + Unpin + Send + 'static,
) -> anyhow::Result<()> {
    tracing::debug!("handle_normalized_logs_ws: starting");
    let mut stream = stream.map_ok(|msg| {
        tracing::debug!("Converting LogMsg to WebSocket message, type: {:?}", std::mem::discriminant(&msg));
        msg.to_ws_message_unchecked()
    });
    let (mut sender, mut receiver) = socket.split();
    tokio::spawn(async move { 
        while let Some(Ok(_)) = receiver.next().await {
            tracing::debug!("Received message from client");
        }
        tracing::debug!("Client receiver loop ended");
    });
    let mut message_count = 0;
    let mut stream_ended = false;
    
    while let Some(item) = stream.next().await {
        match item {
            Ok(msg) => {
                message_count += 1;
                tracing::debug!(%message_count, "Sending WebSocket message to client");
                match sender.send(msg).await {
                    Ok(_) => {
                        // Message sent successfully
                    }
                    Err(e) => {
                        // Check if this is an expected disconnection error
                        let is_expected_disconnection = is_expected_disconnection(&e);
                        
                        if is_expected_disconnection {
                            tracing::debug!(%message_count, "Client disconnected (expected disconnection)");
                        } else {
                            tracing::error!(%message_count, "WebSocket send error: {}", e);
                        }
                        
                        // Stop sending messages regardless of error type
                        stream_ended = true;
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::error!("stream error: {}", e);
                stream_ended = true;
                break;
            }
        }
    }
    
    if stream_ended {
        tracing::debug!(%message_count, "WebSocket stream ended due to error or disconnect");
    } else {
        tracing::debug!(%message_count, "WebSocket stream ended naturally");
    }
    Ok(())
}

pub async fn stop_execution_process(
    Extension(execution_process): Extension<ExecutionProcess>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    deployment
        .container()
        .stop_execution(&execution_process, ExecutionProcessStatus::Killed)
        .await?;

    Ok(ResponseJson(ApiResponse::success(())))
}

pub async fn stream_execution_processes_by_session_ws(
    ws: WebSocketUpgrade,
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<SessionExecutionProcessQuery>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_execution_processes_by_session_ws(
            socket,
            deployment,
            query.session_id,
            query.show_soft_deleted.unwrap_or(false),
        )
        .await
        {
            tracing::warn!("execution processes by session WS closed: {}", e);
        }
    })
}

async fn handle_execution_processes_by_session_ws(
    socket: WebSocket,
    deployment: DeploymentImpl,
    session_id: uuid::Uuid,
    show_soft_deleted: bool,
) -> anyhow::Result<()> {
    // Get the raw stream and convert LogMsg to WebSocket messages
    let mut stream = deployment
        .events()
        .stream_execution_processes_for_session_raw(session_id, show_soft_deleted)
        .await?
        .map_ok(|msg| msg.to_ws_message_unchecked());

    // Split socket into sender and receiver
    let (mut sender, mut receiver) = socket.split();

    // Drain (and ignore) any client->server messages so pings/pongs work
    tokio::spawn(async move { while let Some(Ok(_)) = receiver.next().await {} });

    // Forward server messages with proper disconnection handling
    while let Some(item) = stream.next().await {
        match item {
            Ok(msg) => {
                match sender.send(msg).await {
                    Ok(_) => {
                        // Message sent successfully
                    }
                    Err(e) => {
                        // Check if this is an expected disconnection error
                        let is_expected_disconnection = is_expected_disconnection(&e);
                        
                        if is_expected_disconnection {
                            tracing::debug!("Client disconnected (expected disconnection)");
                        } else {
                            tracing::error!("WebSocket send error: {}", e);
                        }
                        
                        // Stop sending messages regardless of error type
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::error!("stream error: {}", e);
                break;
            }
        }
    }
    Ok(())
}

pub async fn get_execution_process_repo_states(
    Extension(execution_process): Extension<ExecutionProcess>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<ExecutionProcessRepoState>>>, ApiError> {
    let pool = &deployment.db().pool;
    let repo_states =
        ExecutionProcessRepoState::find_by_execution_process_id(pool, execution_process.id).await?;
    Ok(ResponseJson(ApiResponse::success(repo_states)))
}

pub fn router(deployment: &DeploymentImpl) -> Router<DeploymentImpl> {
    let workspace_id_router = Router::new()
        .route("/", get(get_execution_process_by_id))
        .route("/stop", post(stop_execution_process))
        .route("/repo-states", get(get_execution_process_repo_states))
        .route("/raw-logs/ws", get(stream_raw_logs_ws))
        .route("/normalized-logs/ws", get(stream_normalized_logs_ws))
        .layer(from_fn_with_state(
            deployment.clone(),
            load_execution_process_middleware,
        ));

    let workspaces_router = Router::new()
        .route(
            "/stream/session/ws",
            get(stream_execution_processes_by_session_ws),
        )
        .nest("/{id}", workspace_id_router);

    Router::new().nest("/execution-processes", workspaces_router)
}
