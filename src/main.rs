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
//!
//! # Environment Variables
//!
//! Set the "WSS" variable to a WebSocket handshake endpoint.
//! For example:
//!
//! ```bash
//! WSS=ws://localhost:3000/ws cargo run
//! ```
//!
//! If not given, the default is "ws://localhost:3000/ws".
//!
//! (See the implementation of [`route`] for more details.)

use std::{
    sync::{
        atomic::{self, AtomicU64},
        Arc,
    },
    time::Duration,
};

use axum::{
    debug_handler,
    extract::{
        ws::{CloseFrame, Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use sailfish::TemplateOnce;
use tokio::sync::broadcast::{self, error::RecvError};
use tracing::instrument;

/// App state
struct AppState {
    /// Counter
    cnt: AtomicU64,
    /// Sender
    snd: broadcast::Sender<(u64, Message)>,
    /// Where to phone for the WebSocket
    wss: String,
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
            // Say no to a message that's too large
            Race::Me(Some(Ok(Message::Text(t)))) if t.len() > 500 => {
                tracing::warn!("{} sent too long message", id);
                // Warn but don't close
                ws.send(Message::Text("message too long, not sent".to_string()))
                    .await
                    .unwrap();
                // Wait
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            // Send message to others
            Race::Me(Some(Ok(msg @ Message::Text(_)))) => {
                // The magic: send to the broadcast channel
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

/// Serve the index page
#[instrument(skip(state))]
async fn index(State(state): State<Arc<AppState>>) -> Response {
    #[derive(TemplateOnce)]
    #[template(path = "index.html")]
    struct IndexHtml<'a> {
        host: &'a str,
    }

    let index_html = IndexHtml {
        host: state.wss.as_str(),
    };

    Html(index_html.render_once().unwrap()).into_response()
}

fn route() -> Router {
    // Construct state
    let cnt = AtomicU64::new(0);
    let (snd, _rcv) = broadcast::channel(100);
    let wss = std::env::var("WSS").unwrap_or_else(|_| "ws://localhost:3000/ws".into());

    tracing::info!("WSS phone: {}", wss);

    Router::new()
        .route("/", get(index))
        .route("/ws", get(upgrade))
        .with_state(Arc::new(AppState { cnt, snd, wss }))
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
