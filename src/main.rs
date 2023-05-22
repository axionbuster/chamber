use axum::{
    debug_handler,
    extract::{ws::WebSocket, WebSocketUpgrade},
    response::Response,
    routing::get,
    Router,
};
use tracing::instrument;

async fn handle(mut ws: WebSocket) {
    tracing::info!("{ws:?}");
    while let Some(msg) = ws.recv().await {
        match msg {
            Err(e) => tracing::error!("can't receive: {e:?}"),
            Ok(msg) => match ws.send(msg).await {
                Err(e) => tracing::error!("can't reflect: {e:?}"),
                Ok(()) => (),
            },
        }
    }
}

fn fail(e: axum::Error) {
    tracing::error!("failed to accept websocket: {}", e);
}

#[instrument]
#[debug_handler]
async fn upgrade(ws: WebSocketUpgrade) -> Response {
    ws.on_failed_upgrade(fail).on_upgrade(handle)
}

fn route() -> Router {
    Router::new().route("/ws", get(upgrade))
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
