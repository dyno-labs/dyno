//! The build alone on 127.0.0.1, for a browser that dyno drives itself.
//!
//! `dyno check <build> --browser` loads the build in the CDP driver's Chromium. That driver adds
//! the probe and reads the game's requests on its own, so this server is the served run's game
//! server without the probe's tag, the harness page or the run: the same files, content types,
//! byte ranges, `no-store` and path checks. It is on 127.0.0.1, and the CDP driver puts its
//! harness page on `localhost` for a game there, so the game frame stays cross-site.

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use crate::files::{Build, Pages, build_response};
use crate::http::{accept, read_request, write_response};
use crate::{ServeError, listen, url};

/// The address the server binds. Only this machine reaches it.
const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// A build folder served on its own thread, until it is dropped.
#[derive(Debug)]
pub struct BuildServer {
    url: String,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl BuildServer {
    /// Serves this build folder on a free port of 127.0.0.1. A .zip or a missing folder is an
    /// error.
    pub fn start(build: &Path) -> Result<BuildServer, ServeError> {
        let build = Arc::new(Build::open(build)?);
        let listener = listen(IpAddr::V4(LOOPBACK), 0)?;
        let url = url(&LOOPBACK.to_string(), &listener)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| ServeError::Runtime(error.to_string()))?;
        let listener = {
            let _entered = runtime.enter();
            TcpListener::from_std(listener).map_err(|error| ServeError::Bind {
                address: url.clone(),
                detail: error.to_string(),
            })?
        };
        let (stop, stopped) = oneshot::channel();
        let thread = std::thread::spawn(move || {
            runtime.block_on(async move {
                tokio::select! {
                    () = accept(listener, build, serve_file) => {}
                    _ = stopped => {}
                }
            });
        });
        Ok(BuildServer {
            url,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    /// The build's root, such as `http://127.0.0.1:41234/`.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for BuildServer {
    /// Stops serving: the listener closes, and a request still being served is dropped.
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn serve_file(build: Arc<Build>, mut socket: TcpStream) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    let response = build_response(&build, &request, Pages::AsTheyAre);
    write_response(&mut socket, &response, request.method == "HEAD").await;
}
