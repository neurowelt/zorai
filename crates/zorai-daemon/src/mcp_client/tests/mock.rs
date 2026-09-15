use super::super::*;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Notify,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub headers: HashMap<String, String>,
    pub body: Value,
}
pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
    pub headers: Vec<(String, String)>,
}
impl Reply {
    pub fn json(request: &Request, result: Value) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: json!({"jsonrpc":"2.0","id":request.body["id"],"result":result}).to_string(),
            headers: Vec::new(),
        }
    }
    pub fn status(status: u16) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: String::new(),
            headers: Vec::new(),
        }
    }
}
type Handler = Arc<dyn Fn(Request) -> Pin<Box<dyn Future<Output = Reply> + Send>> + Send + Sync>;
pub struct Mock {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    notify: Arc<Notify>,
    cancel: CancellationToken,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Mock {
    pub async fn start<F, Fut>(handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        let handler: Handler = Arc::new(move |request| Box::pin(handler(request)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let notify = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let captured = requests.clone();
        let notices = notify.clone();
        let stopped = cancel.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {_ = stopped.cancelled()=>break,accepted=listener.accept()=>accepted};
                let Ok((mut socket, _)) = accepted else {
                    break;
                };
                let handler = handler.clone();
                let captured = captured.clone();
                let notices = notices.clone();
                let stopped = stopped.clone();
                tokio::spawn(async move {
                    tokio::select! {_ = stopped.cancelled()=>{},_ = async{
                        let mut buffer=Vec::new();let mut chunk=[0;4096];let (end,length,headers,method)=loop{
                            let count=socket.read(&mut chunk).await.unwrap_or(0);if count==0{return;}buffer.extend_from_slice(&chunk[..count]);
                            if let Some(end)=buffer.windows(4).position(|w|w==b"\r\n\r\n"){
                                let text=String::from_utf8_lossy(&buffer[..end]);let mut lines=text.lines();let method=lines.next().unwrap().split_whitespace().next().unwrap().to_owned();
                                let headers:HashMap<String,String>=lines.filter_map(|line|line.split_once(':')).map(|(a,b)|(a.to_ascii_lowercase(),b.trim().to_owned())).collect();
                                let length=headers.get("content-length").and_then(|n|n.parse::<usize>().ok()).unwrap_or(0);break(end+4,length,headers,method);
                            }
                        };
                        while buffer.len()<end+length{let count=socket.read(&mut chunk).await.unwrap_or(0);if count==0{return;}buffer.extend_from_slice(&chunk[..count]);}
                        let body=serde_json::from_slice(&buffer[end..end+length]).unwrap_or(Value::Null);let request=Request{method,headers,body};
                        captured.lock().unwrap().push(request.clone());notices.notify_waiters();
                        let reply=handler(request).await;
                        let mut response=format!("HTTP/1.1 {} Mock\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",reply.status,reply.content_type,reply.body.len());
                        for (name,value) in reply.headers{response.push_str(&format!("{name}: {value}\r\n"));}response.push_str("\r\n");response.push_str(&reply.body);
                        let _=socket.write_all(response.as_bytes()).await;
                    }=>{}}
                });
            }
        });
        Self {
            url,
            requests,
            notify,
            cancel,
        }
    }
    pub fn calls(&self) -> Vec<Request> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.body["method"] == "tools/call")
            .cloned()
            .collect()
    }
    pub async fn wait_calls(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let notified = self.notify.notified();
                if self.calls().len() >= count {
                    return;
                }
                notified.await;
            }
        })
        .await
        .unwrap();
    }
}
pub fn config(mock: &Mock) -> McpServerConfig {
    McpServerConfig {
        id: "mock".into(),
        name: "Local isolated mock".into(),
        url: mock.url.clone(),
        enabled: true,
        share_workspace_context: true,
        ..Default::default()
    }
}
pub fn basic(request: &Request, support: bool) -> Reply {
    if request.method == "GET" {
        return Reply::status(405);
    }
    if request.method == "DELETE" {
        return Reply::status(200);
    }
    match request.body["method"].as_str().unwrap_or("") {
        "initialize" => {
            let capabilities = if support {
                json!({"tools":{},"experimental":{"ai.zorai/workspace-context":{"version":1,"requiredForTools":["consult"]}}})
            } else {
                json!({"tools":{}})
            };
            let mut reply = Reply::json(
                request,
                json!({"protocolVersion":"2025-06-18","capabilities":capabilities,"serverInfo":{"name":"isolated mock","version":"1"},"instructions":"Mock server guidance"}),
            );
            reply
                .headers
                .push(("Mcp-Session-Id".into(), "isolated-session".into()));
            reply
        }
        "tools/list" => {
            if request.body["params"]["cursor"] == "page-two" {
                Reply::json(
                    request,
                    json!({"tools":[{"name":"get_answer","description":"Collect","inputSchema":{"type":"object"}}]}),
                )
            } else {
                Reply::json(
                    request,
                    json!({"tools":[{"name":"consult","description":"Submit","inputSchema":{"type":"object"}},{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}],"nextCursor":"page-two"}),
                )
            }
        }
        "tools/call" => Reply::json(
            request,
            json!({"content":[{"type":"text","text":"ok"}],"structuredContent":{"received":request.body["params"]}}),
        ),
        _ => Reply::status(202),
    }
}
pub async fn connected(manager: &McpManager) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let statuses = manager.statuses();
            if statuses.first().is_some_and(|s| s.state == "connected") {
                break;
            }
            if statuses
                .first()
                .is_some_and(|s| s.state.ends_with("error") || s.state == "offline")
            {
                panic!("connection failed: {statuses:?}");
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("mock connected");
}
pub fn route(manager: &McpManager, original: &str) -> McpToolRoute {
    manager
        .catalog_snapshot()
        .routes
        .values()
        .find(|r| r.original_name == original)
        .unwrap()
        .clone()
}
