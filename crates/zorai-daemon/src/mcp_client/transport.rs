//! Concurrent Streamable HTTP adapter for the official rmcp protocol/service layer.
//! The SDK's 0.8 transport worker waits for each JSON POST; a per-send future here
//! keeps long JSON responses and cancellation independent. POSTs are never replayed.
use super::{McpAuthConfig, McpServerConfig};
use futures::StreamExt;
use rmcp::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::Transport,
    RoleClient,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

const MAX_BODY: usize = 8 * 1024 * 1024;
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub(super) struct HttpError(pub String);

#[derive(Default)]
struct Session {
    id: Option<String>,
    protocol: Option<String>,
}
struct HttpState {
    client: reqwest::Client,
    endpoint: String,
    session: Mutex<Session>,
    incoming: mpsc::Sender<ServerJsonRpcMessage>,
    cancellation: CancellationToken,
    requests: Semaphore,
    gate: Option<DispatchGate>,
    calls: Mutex<HashMap<String, CancellationToken>>,
}
pub(super) type DispatchGate = Arc<dyn Fn(&serde_json::Value) -> Result<(), String> + Send + Sync>;
pub(super) struct HttpTransport {
    state: Arc<HttpState>,
    incoming: mpsc::Receiver<ServerJsonRpcMessage>,
}
impl HttpTransport {
    #[cfg(test)]
    pub fn with_request_limit(mut self, limit: usize) -> Self {
        Arc::get_mut(&mut self.state).unwrap().requests = Semaphore::new(limit);
        self
    }
    pub fn new(
        config: &McpServerConfig,
        secret: Option<&str>,
        gate: Option<DispatchGate>,
    ) -> Result<Self, String> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json, text/event-stream"),
        );
        let header = match &config.auth {
            McpAuthConfig::None => None,
            McpAuthConfig::Bearer { .. } => Some((
                reqwest::header::AUTHORIZATION,
                format!(
                    "Bearer {}",
                    secret.ok_or("MCP bearer credential is missing")?
                ),
            )),
            McpAuthConfig::ApiKey { header, .. } => Some((
                reqwest::header::HeaderName::from_bytes(header.as_bytes())
                    .map_err(|_| "Invalid MCP API-key header")?,
                secret
                    .ok_or("MCP API-key credential is missing")?
                    .to_owned(),
            )),
        };
        if let Some((name, value)) = header {
            let mut value = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| "Invalid MCP credential header")?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(1800))
            .build()
            .map_err(|_| "Cannot construct MCP HTTP client")?;
        let (incoming, rx) = mpsc::channel(128);
        Ok(Self {
            state: Arc::new(HttpState {
                client,
                endpoint: config.url.clone(),
                session: Mutex::new(Session::default()),
                incoming,
                cancellation: CancellationToken::new(),
                requests: Semaphore::new(64),
                gate,
                calls: Mutex::new(HashMap::new()),
            }),
            incoming: rx,
        })
    }
}
impl Transport<RoleClient> for HttpTransport {
    type Error = HttpError;
    fn send(
        &mut self,
        item: ClientJsonRpcMessage,
    ) -> impl std::future::Future<Output = Result<(), HttpError>> + Send + 'static {
        let state = self.state.clone();
        let value = serde_json::to_value(&item).expect("MCP message serialization");
        // Responses use the server's ID namespace, which may overlap client requests.
        let request_id = value
            .get("id")
            .filter(|_| value.get("method").is_some())
            .map(ValueId::key);
        let request_cancel = {
            let mut calls = state.calls.lock().unwrap();
            if value["method"] == "notifications/cancelled" {
                if let Some(id) = value["params"].get("requestId") {
                    if let Some(cancel) = calls.get(&ValueId::key(id)) {
                        cancel.cancel();
                    }
                }
            }
            request_id
                .as_ref()
                .map(|id| calls.entry(id.clone()).or_default().clone())
        };
        async move {
            let result = tokio::select! {
                biased;
                _ = state.cancellation.cancelled() => Err(HttpError("MCP connection closed".into())),
                _ = async { if let Some(cancel) = &request_cancel { cancel.cancelled().await; } else { std::future::pending::<()>().await; } } => Err(HttpError("MCP_CANCELLED: MCP call cancelled; any queued submission was discarded. An already submitted request may have an unknown outcome; it was not replayed.".into())),
                result = post(state.clone(),item) => result,
            };
            if let Some(id) = request_id {
                state.calls.lock().unwrap().remove(&id);
            }
            result
        }
    }
    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        self.incoming.recv().await
    }
    async fn close(&mut self) -> Result<(), HttpError> {
        self.state.cancellation.cancel();
        let session = self.state.session.lock().unwrap().id.clone();
        if let Some(session) = session {
            let request =
                request(&self.state, reqwest::Method::DELETE).header("Mcp-Session-Id", session);
            let _ = tokio::time::timeout(Duration::from_secs(2), request.send()).await;
        }
        self.incoming.close();
        Ok(())
    }
}
struct ValueId;
impl ValueId {
    fn key(value: &serde_json::Value) -> String {
        value.to_string()
    }
}
impl Drop for HttpTransport {
    fn drop(&mut self) {
        self.state.cancellation.cancel();
    }
}
fn request(state: &HttpState, method: reqwest::Method) -> reqwest::RequestBuilder {
    let session = state.session.lock().unwrap();
    let mut request = state.client.request(method, &state.endpoint);
    if let Some(id) = &session.id {
        request = request.header("Mcp-Session-Id", id);
    }
    if let Some(version) = &session.protocol {
        request = request.header("MCP-Protocol-Version", version);
    }
    request
}
async fn post(state: Arc<HttpState>, item: ClientJsonRpcMessage) -> Result<(), HttpError> {
    // Notifications, especially cancellation, do not wait behind long business calls.
    let value =
        serde_json::to_value(&item).map_err(|_| HttpError("Cannot encode MCP request".into()))?;
    let initialize = value["method"] == "initialize";
    let initialized = value["method"] == "notifications/initialized";
    let _permit = if value.get("id").is_some() && value.get("method").is_some() {
        Some(
            state
                .requests
                .acquire()
                .await
                .map_err(|_| HttpError("MCP connection closed".into()))?,
        )
    } else {
        None
    };
    if let Some(gate) = &state.gate {
        gate(&value).map_err(HttpError)?;
    }
    let response = request(&state, reqwest::Method::POST)
        .json(&item)
        .send()
        .await
        .map_err(classify_reqwest)?;
    check_status(&response)?;
    if initialize {
        if let Some(id) = response.headers().get("mcp-session-id") {
            let id = id
                .to_str()
                .map_err(|_| HttpError("MCP protocol error: invalid session header".into()))?;
            if id.is_empty() || id.len() > 1024 {
                return Err(HttpError(
                    "MCP protocol error: invalid session header".into(),
                ));
            }
            state.session.lock().unwrap().id = Some(id.to_owned());
        }
    }
    consume(&state, response, initialize, value.get("id").cloned()).await?;
    if initialized && state.session.lock().unwrap().id.is_some() {
        let state = state.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = state.cancellation.cancelled() => {},
                _ = async {
                    // GET is optional in Streamable HTTP; 405 means no standalone stream.
                    if let Ok(response) = request(&state,reqwest::Method::GET).send().await {
                        if response.status().is_success() { let _ = consume(&state,response,false,None).await; }
                    }
                } => {},
            }
        });
    }
    Ok(())
}
fn classify_reqwest(error: reqwest::Error) -> HttpError {
    HttpError(if error.is_timeout() { "MCP request timed out; remote outcome may be unknown. Reconnect before retrying; the request was not replayed." } else { "MCP server offline or HTTP connection failed; remote outcome may be unknown. The request was not replayed." }.into())
}
fn check_status(response: &reqwest::Response) -> Result<(), HttpError> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    Err(HttpError(match status.as_u16() {
        401 | 403 => "MCP authentication failed; update credentials in Settings → MCP".into(),
        404 => "MCP session or endpoint was lost (HTTP 404); reconnect. The request was not replayed.".into(),
        300..=399 => "MCP redirect refused; configure the final endpoint explicitly. Credentials and context were not forwarded.".into(),
        _ => format!("MCP HTTP error {}. The request was not replayed.",status.as_u16()),
    }))
}
async fn emit(
    state: &HttpState,
    value: serde_json::Value,
    initialize: bool,
) -> Result<(), HttpError> {
    if initialize {
        if let Some(version) = value["result"]["protocolVersion"].as_str() {
            if !matches!(version, "2025-03-26" | "2025-06-18") {
                return Err(HttpError("MCP protocol error: unsupported negotiated version (supported: 2025-03-26, 2025-06-18)".into()));
            }
            state.session.lock().unwrap().protocol = Some(version.to_owned());
        }
    }
    let message = serde_json::from_value(value)
        .map_err(|_| HttpError("MCP protocol error: invalid JSON-RPC response".into()))?;
    state
        .incoming
        .send(message)
        .await
        .map_err(|_| HttpError("MCP connection closed".into()))
}
async fn consume(
    state: &HttpState,
    response: reqwest::Response,
    initialize: bool,
    expected_id: Option<serde_json::Value>,
) -> Result<(), HttpError> {
    if response.status() == reqwest::StatusCode::ACCEPTED
        || response.status() == reqwest::StatusCode::NO_CONTENT
    {
        return Ok(());
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let sse = content_type.starts_with("text/event-stream");
    if !sse && !content_type.starts_with("application/json") {
        return Err(HttpError(
            "MCP protocol error: expected JSON or SSE response".into(),
        ));
    }
    let mut bytes = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut data = String::new();
    while let Some(chunk) = bytes.next().await {
        let chunk = chunk.map_err(classify_reqwest)?;
        buffer.extend_from_slice(&chunk);
        if buffer.len() + data.len() > MAX_BODY {
            return Err(HttpError(
                "MCP response exceeds the 8 MiB safety limit".into(),
            ));
        }
        if sse {
            while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                let line = buffer.drain(..=end).collect::<Vec<_>>();
                let line = std::str::from_utf8(&line)
                    .map_err(|_| HttpError("MCP SSE response is not UTF-8".into()))?
                    .trim_end_matches(['\r', '\n']);
                if line.is_empty() {
                    if !data.is_empty() {
                        let value: serde_json::Value =
                            serde_json::from_str(&data).map_err(|_| {
                                HttpError("MCP SSE response contains invalid JSON".into())
                            })?;
                        let done = expected_id.as_ref().is_some_and(|id| {
                            value.get("id") == Some(id)
                                && (value.get("result").is_some() || value.get("error").is_some())
                        });
                        emit(state, value, initialize).await?;
                        data.clear();
                        if done {
                            return Ok(());
                        }
                    }
                } else if let Some(line) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(line.strip_prefix(' ').unwrap_or(line));
                }
            }
        }
    }
    if !sse {
        let value = serde_json::from_slice(&buffer)
            .map_err(|_| HttpError("MCP protocol error: invalid JSON response".into()))?;
        emit(state, value, initialize).await?;
    } else if expected_id.is_some() {
        return Err(HttpError("MCP SSE stream ended before the result; remote outcome unknown. The request was not replayed.".into()));
    }
    Ok(())
}
