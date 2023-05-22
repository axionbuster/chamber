//! Implement an "echo chamber": a chat room where every message is relayed to
//! themselves and every other user.
//!
//! # Behavior
//!
//! - A user connects to the server
//! - Reply: "You are &lt;id&gt;"
//! - The user sends a message
//! - Reply (everyone): "&lt;id&gt; says &lt;msg&gt;"
//! - (This includes the user themselves)
//! - The user sends binary data
//! - Reply: closing with "only text messages are allowed"
//! - The user disconnects
//! - Reply (everyone): "&lt;id&gt; disconnected"
//! - The user is set to be receiving binary data
//! - (Filtered out, not received)

use std::sync::{
    atomic::{self, AtomicU64},
    Arc,
};

use axum::{
    debug_handler,
    extract::{
        ws::{CloseFrame, Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::Response,
    routing::get,
    Router,
};
use tokio::sync::broadcast::{self, error::RecvError};
use tracing::instrument;

/// App state
struct AppState {
    /// Counter
    cnt: AtomicU64,
    /// Sender
    snd: broadcast::Sender<(u64, Message)>,
}

#[instrument(skip(ws, state))]
async fn handle(mut ws: WebSocket, state: Arc<AppState>, id: u64) {
    enum Race {
        Other(Result<(u64, Message), RecvError>),
        Me(Option<Result<Message, axum::Error>>),
    }

    tracing::info!("admitted");

    ws.send(Message::Text(format!("You are {}", id)))
        .await
        .unwrap();

    // Subscribe to the room
    let mut rcv = state.snd.subscribe();

    loop {
        // Competitively receive a message from either process
        let evt = tokio::select! {
            v = rcv.recv() => Race::Other(v),
            v = ws.recv() => Race::Me(v),
        };

        match evt {
            // If closed, break
            Race::Other(Err(RecvError::Closed)) => break,
            // If lagged, log, ignore
            Race::Other(Err(RecvError::Lagged(skip))) => {
                tracing::warn!("{} lagged {} messages", id, skip)
            }
            // Special message (broadcast to all)
            Race::Other(Ok((u64::MAX, msg))) => {
                ws.send(msg).await.unwrap();
            }
            // Relay messages from others
            Race::Other(Ok((id2, Message::Text(msg)))) => {
                let msg = format!("{} says {}", id2, msg);
                let msg = Message::Text(msg);
                ws.send(msg).await.unwrap();
            }
            // Ignore binary
            Race::Other(Ok((_, _))) => (),
            // Connection closed
            Race::Me(None) => break,
            // Handle errors
            Race::Me(Some(Err(e))) => tracing::error!("msg error {e:?}"),
            // Send message to others
            Race::Me(Some(Ok(msg @ Message::Text(_)))) => {
                state.snd.send((id, msg)).unwrap();
            }
            // Reject binary messages, close connection
            // Don't react to close messages---it may result in sending to a closed websocket
            Race::Me(Some(Ok(msg))) if !matches!(msg, Message::Close(_)) => {
                tracing::warn!("{} sent binary", id);
                let msg = CloseFrame {
                    // 1003: unsupported data
                    code: 1003,
                    reason: "only text messages are allowed".into(),
                };
                ws.send(Message::Close(Some(msg))).await.unwrap();
                // (connection will have been closed by now)
                break;
            }
            Race::Me(_) => (),
        }
    }

    // Send special message
    state
        .snd
        .send((u64::MAX, Message::Text(format!("{} disconnected", id))))
        .unwrap();
    tracing::info!("{} disconnected", id);
}

fn fail(e: axum::Error) {
    tracing::error!("failed to accept websocket: {}", e);
}

#[instrument(skip(ws, state))]
#[debug_handler]
async fn upgrade(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_failed_upgrade(fail).on_upgrade(|socket| {
        // SeqCst: ensure every thread agrees on the value of the counter
        // Increment & get OLD value
        let id = state.cnt.fetch_add(1, atomic::Ordering::SeqCst);
        handle(socket, state, id)
    })
}

fn route() -> Router {
    // Construct state
    let cnt = AtomicU64::new(0);
    let (snd, _rcv) = broadcast::channel(100);

    Router::new()
        .route("/ws", get(upgrade))
        .with_state(Arc::new(AppState { cnt, snd }))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let bind: std::net::SocketAddr = "0.0.0.0:3000".parse().unwrap();
    let r = route();
    let s = axum::Server::bind(&bind).serve(r.into_make_service());
    tracing::info!("Greetings from {}", bind);
    s.await.unwrap();
}
