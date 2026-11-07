//! The session layer: one WebSocket to the browser, and many flat CDP sessions on it.
//!
//! Each call gets a connection-wide id and, for a target, its `sessionId`. A reply resolves its
//! call. An event goes to one channel with its top-level `sessionId`, and the run routes it. When
//! the browser detaches a session, the calls waiting on it fail at once.
//!
//! The envelope is our own: `chromiumoxide_types` reads `session_id` in snake case, so it never
//! sees the `sessionId` of flat mode (CDP feasibility test).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::CdpError;

/// One event from the browser. `session_id` is None for the browser session.
#[derive(Clone, Debug)]
pub struct Event {
    pub session_id: Option<String>,
    pub method: String,
    pub params: Value,
}

/// Every incoming message, a reply or an event.
#[derive(Deserialize)]
struct Envelope {
    id: Option<u64>,
    method: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default)]
    params: Value,
    result: Option<Value>,
    error: Option<Value>,
}

/// A call that waits for its reply.
struct Pending {
    session_id: Option<String>,
    method: String,
    reply: oneshot::Sender<Result<Value, CdpError>>,
}

/// The connection to one browser.
pub struct Connection {
    outgoing: mpsc::UnboundedSender<Message>,
    pending: Mutex<HashMap<u64, Pending>>,
    next_id: AtomicU64,
    call_timeout: Duration,
}

/// The event a target sends when the browser detaches its session.
const DETACHED: &str = "Target.detachedFromTarget";

impl Connection {
    /// Connects to the browser's WebSocket and starts the reader and writer tasks. Every event
    /// arrives on the returned channel.
    pub async fn open(
        web_socket_url: &str,
        call_timeout: Duration,
    ) -> Result<(Arc<Connection>, mpsc::UnboundedReceiver<Event>), CdpError> {
        let (socket, _) = tokio_tungstenite::connect_async(web_socket_url)
            .await
            .map_err(|error| CdpError::Connect(error.to_string()))?;
        let (mut sink, mut stream) = socket.split();
        let (outgoing, mut to_send) = mpsc::unbounded_channel::<Message>();
        let (events, received) = mpsc::unbounded_channel::<Event>();
        let connection = Arc::new(Connection {
            outgoing,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            call_timeout,
        });
        tokio::spawn(async move {
            while let Some(message) = to_send.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });
        let reader = Arc::clone(&connection);
        tokio::spawn(async move {
            while let Some(Ok(message)) = stream.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                // A message we cannot read is dropped: it answers no call we made.
                if let Ok(envelope) = serde_json::from_str::<Envelope>(&text) {
                    reader.dispatch(envelope, &events);
                }
            }
            reader.fail_all("the browser closed the connection");
        });
        Ok((connection, received))
    }

    /// Sends one call and waits for its reply, up to the call timeout.
    pub async fn call(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        self.pending_calls().insert(
            id,
            Pending {
                session_id: session_id.map(str::to_owned),
                method: method.to_owned(),
                reply,
            },
        );
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session_id) = session_id {
            message["sessionId"] = json!(session_id);
        }
        if self
            .outgoing
            .send(Message::text(message.to_string()))
            .is_err()
        {
            self.pending_calls().remove(&id);
            return Err(CdpError::call(method, "the connection is closed"));
        }
        match tokio::time::timeout(self.call_timeout, answer).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(CdpError::call(method, "the reply was dropped")),
            Err(_) => {
                self.pending_calls().remove(&id);
                Err(CdpError::call(method, "no reply in time"))
            }
        }
    }

    fn dispatch(&self, envelope: Envelope, events: &mpsc::UnboundedSender<Event>) {
        if let Some(id) = envelope.id {
            let Some(pending) = self.pending_calls().remove(&id) else {
                return;
            };
            let reply = match envelope.error {
                Some(error) => Err(CdpError::call(&pending.method, &error.to_string())),
                None => Ok(envelope.result.unwrap_or(Value::Null)),
            };
            // The caller may have given up already.
            let _ = pending.reply.send(reply);
            return;
        }
        let Some(method) = envelope.method else {
            return;
        };
        if method == DETACHED
            && let Some(gone) = envelope.params["sessionId"].as_str()
        {
            self.fail_session(gone);
        }
        // The run may have finished and dropped the receiver.
        let _ = events.send(Event {
            session_id: envelope.session_id,
            method,
            params: envelope.params,
        });
    }

    fn fail_session(&self, session_id: &str) {
        let mut pending = self.pending_calls();
        let ids: Vec<u64> = pending
            .iter()
            .filter(|(_, call)| call.session_id.as_deref() == Some(session_id))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(call) = pending.remove(&id) {
                let _ = call
                    .reply
                    .send(Err(CdpError::call(&call.method, "the session detached")));
            }
        }
    }

    fn fail_all(&self, why: &str) {
        for (_, call) in self.pending_calls().drain() {
            let _ = call.reply.send(Err(CdpError::call(&call.method, why)));
        }
    }

    /// The waiting calls. A panic while the lock is held leaves the map whole, so the poison is
    /// ignored.
    fn pending_calls(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Pending>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One flat session. An `id` of None is the browser session.
#[derive(Clone)]
pub struct Session {
    connection: Arc<Connection>,
    id: Option<String>,
}

impl Session {
    pub fn browser(connection: &Arc<Connection>) -> Session {
        Session {
            connection: Arc::clone(connection),
            id: None,
        }
    }

    /// The session of a target that the browser attached.
    pub fn child(&self, id: &str) -> Session {
        Session {
            connection: Arc::clone(&self.connection),
            id: Some(id.to_owned()),
        }
    }

    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Sends a call on this session and waits for its reply.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
        self.connection
            .call(self.id.as_deref(), method, params)
            .await
    }
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;

    /// Long enough that a call which fails before it means the failure came at once.
    const LONG: Duration = Duration::from_secs(60);

    /// What the fake browser does with one call.
    enum Answer {
        Send(Vec<Value>),
        Close,
    }

    /// A fake browser on 127.0.0.1 that answers each call it reads with `answer`, and the
    /// connection to it.
    async fn connect(
        call_timeout: Duration,
        answer: impl Fn(&Value) -> Answer + Send + 'static,
    ) -> (Arc<Connection>, mpsc::UnboundedReceiver<Event>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let call: Value = serde_json::from_str(&text).unwrap();
                match answer(&call) {
                    Answer::Send(messages) => {
                        for message in messages {
                            let text = match message {
                                Value::String(raw) => raw,
                                message => message.to_string(),
                            };
                            socket.send(Message::text(text)).await.unwrap();
                        }
                    }
                    Answer::Close => {
                        let _ = socket.close(None).await;
                        return;
                    }
                }
            }
        });
        Connection::open(&url, call_timeout).await.unwrap()
    }

    fn reply(call: &Value, result: Value) -> Value {
        json!({ "id": call["id"], "result": result })
    }

    fn detail(error: CdpError) -> (String, String) {
        match error {
            CdpError::Call { method, detail } => (method, detail),
            other => panic!("not a call error: {other}"),
        }
    }

    #[tokio::test]
    async fn a_reply_resolves_its_call() {
        let (connection, _events) = connect(LONG, |call| {
            Answer::Send(vec![reply(call, json!({ "method": call["method"] }))])
        })
        .await;
        let result = Session::browser(&connection)
            .call("Browser.getVersion", json!({}))
            .await
            .unwrap();
        assert_eq!(result, json!({ "method": "Browser.getVersion" }));
    }

    #[tokio::test]
    async fn a_call_on_a_target_carries_its_session_id_and_the_browser_call_none() {
        let (connection, _events) = connect(LONG, |call| {
            Answer::Send(vec![reply(call, call["sessionId"].clone())])
        })
        .await;
        let browser = Session::browser(&connection);
        let target = browser.child("S1");
        assert_eq!(target.id(), Some("S1"));
        assert_eq!(target.call("Page.enable", json!({})).await.unwrap(), "S1");
        assert_eq!(
            browser.call("Target.getTargets", json!({})).await.unwrap(),
            Value::Null
        );
    }

    #[tokio::test]
    async fn an_error_reply_fails_the_call_and_names_its_method() {
        let (connection, _events) = connect(LONG, |call| {
            Answer::Send(vec![
                json!({ "id": call["id"], "error": { "message": "No frame" } }),
            ])
        })
        .await;
        let error = Session::browser(&connection)
            .call("Page.navigate", json!({}))
            .await
            .unwrap_err();
        let (method, detail) = detail(error);
        assert_eq!(method, "Page.navigate");
        assert!(detail.contains("No frame"), "{detail}");
    }

    #[tokio::test]
    async fn a_reply_with_no_result_is_null() {
        let (connection, _events) =
            connect(LONG, |call| Answer::Send(vec![json!({ "id": call["id"] })])).await;
        let result = Session::browser(&connection)
            .call("Page.enable", json!({}))
            .await;
        assert_eq!(result.unwrap(), Value::Null);
    }

    #[tokio::test]
    async fn a_call_with_no_reply_fails_after_the_call_timeout() {
        let (connection, _events) =
            connect(Duration::from_millis(50), |_| Answer::Send(Vec::new())).await;
        let error = Session::browser(&connection)
            .call("Runtime.evaluate", json!({}))
            .await
            .unwrap_err();
        assert_eq!(detail(error).1, "no reply in time");
    }

    #[tokio::test]
    async fn a_detached_session_fails_its_waiting_calls_at_once() {
        let (connection, mut events) = connect(LONG, |call| {
            Answer::Send(vec![json!({
                "method": DETACHED,
                "params": { "sessionId": call["sessionId"] },
            })])
        })
        .await;
        let error = Session::browser(&connection)
            .child("S1")
            .call("Runtime.evaluate", json!({}))
            .await
            .unwrap_err();
        assert_eq!(detail(error).1, "the session detached");
        // The run still sees the event.
        assert_eq!(events.recv().await.unwrap().method, DETACHED);
    }

    #[tokio::test]
    async fn a_detached_session_leaves_the_calls_of_other_sessions_waiting() {
        let (connection, _events) = connect(LONG, |call| {
            let detach = json!({ "method": DETACHED, "params": { "sessionId": "other" } });
            Answer::Send(vec![detach, reply(call, json!("still here"))])
        })
        .await;
        let result = Session::browser(&connection)
            .child("S1")
            .call("Runtime.evaluate", json!({}))
            .await;
        assert_eq!(result.unwrap(), "still here");
    }

    #[tokio::test]
    async fn a_closed_connection_fails_its_waiting_calls() {
        let (connection, _events) = connect(LONG, |_| Answer::Close).await;
        let error = Session::browser(&connection)
            .call("Page.navigate", json!({}))
            .await
            .unwrap_err();
        assert_eq!(detail(error).1, "the browser closed the connection");
    }

    #[tokio::test]
    async fn an_event_arrives_with_its_session_id_and_params() {
        let (connection, mut events) = connect(LONG, |call| {
            let event = json!({
                "method": "Page.loadEventFired",
                "sessionId": "S1",
                "params": { "timestamp": 1.5 },
            });
            Answer::Send(vec![event, reply(call, json!({}))])
        })
        .await;
        Session::browser(&connection)
            .call("Page.enable", json!({}))
            .await
            .unwrap();
        let event = events.recv().await.unwrap();
        assert_eq!(event.session_id.as_deref(), Some("S1"));
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.params, json!({ "timestamp": 1.5 }));
    }

    #[tokio::test]
    async fn messages_that_answer_no_call_are_dropped() {
        let (connection, mut events) = connect(LONG, |call| {
            Answer::Send(vec![
                json!("not json"),
                json!({ "id": 9999, "result": "for no call" }),
                json!({ "result": "no id and no method" }),
                reply(call, json!("the reply")),
            ])
        })
        .await;
        let result = Session::browser(&connection)
            .call("Page.enable", json!({}))
            .await;
        assert_eq!(result.unwrap(), "the reply");
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_browser_that_does_not_listen_cannot_be_connected_to() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        drop(listener);
        let error = Connection::open(&url, LONG).await.err().unwrap();
        assert!(matches!(error, CdpError::Connect(_)), "{error}");
    }
}
