#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use sppy_server::{ServerConfig, ServerHandle};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

pub fn test_config() -> ServerConfig {
    ServerConfig { listen: "127.0.0.1:0".parse().unwrap(), ..ServerConfig::default() }
}

pub async fn start_server(dir: &Path, config: ServerConfig) -> ServerHandle {
    sppy_server::start(config, dir.to_path_buf()).await.expect("server starts")
}

/// TCP proxy in front of the server so tests can cut connections without the server noticing.
pub struct Proxy {
    pub addr: SocketAddr,
    conns: Arc<Mutex<Vec<JoinHandle<()>>>>,
    accept: JoinHandle<()>,
}

impl Proxy {
    pub async fn start(upstream: SocketAddr) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let conns: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();
        let c = conns.clone();
        let accept = tokio::spawn(async move {
            while let Ok((mut down, _)) = listener.accept().await {
                let task = tokio::spawn(async move {
                    if let Ok(mut up) = TcpStream::connect(upstream).await {
                        let _ = tokio::io::copy_bidirectional(&mut down, &mut up).await;
                    }
                });
                c.lock().unwrap().push(task);
            }
        });
        Proxy { addr, conns, accept }
    }

    /// Drop every proxied connection (both sides see a reset/EOF).
    pub fn cut(&self) {
        for t in self.conns.lock().unwrap().drain(..) {
            t.abort();
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.cut();
        self.accept.abort();
    }
}
