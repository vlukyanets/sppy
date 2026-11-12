#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use sppy_proto::*;
use sppy_server::{ServerConfig, ServerHandle};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub const TIMEOUT: Duration = Duration::from_secs(2);

pub fn test_config() -> ServerConfig {
    ServerConfig { listen: "127.0.0.1:0".parse().unwrap(), ..ServerConfig::default() }
}

pub async fn start_server(dir: &Path, config: ServerConfig) -> ServerHandle {
    sppy_server::start(config, dir.to_path_buf()).await.expect("server starts")
}

pub struct TestClient {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_req: u32,
}

impl TestClient {
    pub async fn connect_raw(addr: SocketAddr) -> TestClient {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
        TestClient { ws, next_req: 1 }
    }

    pub async fn hello(addr: SocketAddr, auth: Auth, nickname: &str) -> (TestClient, ServerMsg) {
        let mut c = Self::connect_raw(addr).await;
        c.send(ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            room: RoomId::main(),
            auth,
            nickname: nickname.into(),
            client: "test/0".into(),
        })
        .await;
        let first = c.recv().await;
        (c, first)
    }

    /// Connect and expect a Welcome.
    pub async fn join(addr: SocketAddr, auth: Auth, nickname: &str) -> (TestClient, Welcome) {
        let (c, first) = Self::hello(addr, auth, nickname).await;
        match first {
            ServerMsg::Welcome(w) => (c, w),
            other => panic!("expected Welcome, got {other:?}"),
        }
    }

    pub fn req(&mut self) -> u32 {
        self.next_req += 1;
        self.next_req
    }

    pub async fn send(&mut self, msg: ClientMsg) {
        self.ws.send(Message::text(encode(&msg))).await.unwrap();
    }

    pub async fn send_raw(&mut self, text: &str) {
        self.ws.send(Message::text(text.to_owned())).await.unwrap();
    }

    /// Next server message; panics after [`TIMEOUT`] or when the socket closes.
    pub async fn recv(&mut self) -> ServerMsg {
        self.try_recv().await.expect("socket closed while waiting for a message")
    }

    /// Next server message, or None when the socket is closed.
    pub async fn try_recv(&mut self) -> Option<ServerMsg> {
        loop {
            let frame = tokio::time::timeout(TIMEOUT, self.ws.next()).await.expect("timed out");
            match frame {
                Some(Ok(Message::Text(t))) => return Some(decode_server(t.as_str()).unwrap()),
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
                Some(Ok(_)) => continue,
            }
        }
    }

    pub async fn recv_until(&mut self, pred: impl Fn(&ServerMsg) -> bool) -> ServerMsg {
        loop {
            let m = self.recv().await;
            if pred(&m) {
                return m;
            }
        }
    }

    /// Drain messages until the socket closes; returns everything received.
    pub async fn recv_until_closed(&mut self) -> Vec<ServerMsg> {
        let mut out = Vec::new();
        while let Some(m) = self.try_recv().await {
            out.push(m);
        }
        out
    }
}
