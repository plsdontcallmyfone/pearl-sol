//! HTTP JSON-RPC (POST /) and WebSocket (with `subscribe`) servers. Both forward every
//! call to the node actor's mailbox; the actor owns all state.

use std::net::SocketAddr;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::dispatch::RpcError;

pub enum ApiRequest {
    Call { method: String, params: Value, reply: oneshot::Sender<Result<Value, RpcError>> },
    Subscribe { topics: Vec<String>, tx: mpsc::UnboundedSender<String> },
}

pub type Mailbox = mpsc::Sender<ApiRequest>;

pub const TOPICS: &[&str] = &["heads", "finalized", "txs", "withdrawals"];

async fn call(mut mb: Mailbox, method: String, params: Value) -> Result<Value, RpcError> {
    let (tx, rx) = oneshot::channel();
    mb.send(ApiRequest::Call { method, params, reply: tx }).await.map_err(|_| RpcError::server("node is shutting down"))?;
    rx.await.map_err(|_| RpcError::server("node dropped the request"))?
}

/// One JSON-RPC request object to a response object.
async fn handle_one(mb: Mailbox, req: Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = req.get("method").and_then(|m| m.as_str()).map(String::from) else {
        return json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32600, "message": "missing method"}});
    };
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    match call(mb, method, params).await {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": e.code, "message": e.message}}),
    }
}

async fn http_rpc(State(mb): State<Mailbox>, Json(body): Json<Value>) -> Json<Value> {
    match body {
        Value::Array(reqs) => {
            let mut out = Vec::with_capacity(reqs.len());
            for r in reqs {
                out.push(handle_one(mb.clone(), r).await);
            }
            Json(Value::Array(out))
        }
        r => Json(handle_one(mb, r).await),
    }
}

pub async fn serve_http(addr: SocketAddr, mb: Mailbox) -> std::io::Result<()> {
    let app = Router::new()
        .route("/", post(http_rpc))
        .route("/health", get(|| async { "ok" }))
        // a putBlob of the largest blob (blobs::MAX_BLOB_BYTES_LIMIT, 7 MiB) is about 9.4 MiB of
        // base64 JSON; axum's default 2 MiB limit refused every blob over about 1.5 MiB
        .layer(axum::extract::DefaultBodyLimit::max(12 << 20))
        .with_state(mb);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "HTTP JSON-RPC listening");
    axum::serve(listener, app).await
}

async fn ws_upgrade(State(mb): State<Mailbox>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session(socket, mb))
}

async fn ws_session(mut socket: WebSocket, mb: Mailbox) {
    let (ntx, mut nrx) = mpsc::unbounded::<String>();
    let mut subscribed = false;
    loop {
        tokio::select! {
            msg = socket.recv() => {
                let Some(Ok(msg)) = msg else { break };
                let Message::Text(text) = msg else { continue };
                let req: Value = match serde_json::from_str(text.as_str()) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = socket.send(Message::Text(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":e.to_string()}}).to_string().into())).await;
                        continue;
                    }
                };
                let resp = if req.get("method").and_then(|m| m.as_str()) == Some("subscribe") {
                    let id = req.get("id").cloned().unwrap_or(Value::Null);
                    let topics: Vec<String> = req
                        .pointer("/params/topics")
                        .and_then(|t| t.as_array())
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                        .unwrap_or_else(|| TOPICS.iter().map(|s| s.to_string()).collect());
                    if let Some(bad) = topics.iter().find(|t| !TOPICS.contains(&t.as_str())) {
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":format!("unknown topic {bad}")}})
                    } else {
                        let mut m = mb.clone();
                        let ok = m.send(ApiRequest::Subscribe { topics: topics.clone(), tx: ntx.clone() }).await.is_ok();
                        subscribed |= ok;
                        json!({"jsonrpc":"2.0","id":id,"result":{"subscribed": topics}})
                    }
                } else {
                    handle_one(mb.clone(), req).await
                };
                if socket.send(Message::Text(resp.to_string().into())).await.is_err() {
                    break;
                }
            }
            n = nrx.next(), if subscribed => {
                let Some(n) = n else { break };
                if socket.send(Message::Text(n.into())).await.is_err() {
                    break;
                }
            }
        }
    }
}

pub async fn serve_ws(addr: SocketAddr, mb: Mailbox) -> std::io::Result<()> {
    let app = Router::new().route("/", get(ws_upgrade)).with_state(mb);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "WebSocket JSON-RPC listening");
    if let Err(e) = axum::serve(listener, app).await {
        warn!("ws server: {e}");
        return Err(e);
    }
    Ok(())
}
