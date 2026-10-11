use std::{
    collections::BTreeMap,
    io::{BufReader, Read},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{Capabilities, read_frame, uri, write_frame};
use serde_json::{Value, json};

type Response = Result<Value, String>;
type Pending = Arc<Mutex<BTreeMap<u64, Sender<Response>>>>;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub root: PathBuf,
    pub settings: Value,
}

#[derive(Clone, Debug)]
pub enum Event {
    Notification { method: String, params: Value },
    Stderr(String),
    Exited(String),
}

struct Transport {
    writer: Sender<Value>,
    pending: Pending,
    next_id: AtomicU64,
    failure: Arc<Mutex<Option<String>>>,
}

impl Transport {
    fn send(&self, message: Value) -> Result<(), String> {
        if let Some(error) = &*self.failure.lock().unwrap() {
            return Err(error.clone());
        }
        self.writer.send(message).map_err(|_| "Language-server input closed".into())
    }

    fn request(self: &Arc<Self>, method: &str, params: Value) -> Result<Request, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, sender);
        if let Err(error) = self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})) {
            self.pending.lock().unwrap().remove(&id);
            return Err(error);
        }
        Ok(Request {
            id,
            receiver,
            transport: Arc::clone(self),
            finished: false,
            started: Instant::now(),
        })
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}))
    }
}

pub struct Request {
    id: u64,
    receiver: Receiver<Response>,
    transport: Arc<Transport>,
    finished: bool,
    started: Instant,
}

impl Request {
    pub fn poll(&mut self) -> Option<Response> {
        if self.finished {
            return None;
        }
        let result = match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) if self.started.elapsed() < Duration::from_secs(10) => None,
            Err(TryRecvError::Empty) => {
                self.cancel();
                Some(Err("Language-server request timed out".into()))
            }
            Err(TryRecvError::Disconnected) => Some(Err("Language server exited with a pending request".into())),
        };
        if result.is_some() {
            self.finished = true;
        }
        result
    }

    fn wait(&mut self, timeout: Duration) -> Response {
        let result = self
            .receiver
            .recv_timeout(timeout)
            .map_err(|error| format!("Language-server request failed: {error}"))?;
        self.finished = true;
        result
    }

    fn cancel(&mut self) {
        if !self.finished {
            self.transport.pending.lock().unwrap().remove(&self.id);
            let _ = self.transport.notify("$/cancelRequest", json!({"id":self.id}));
            self.finished = true;
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct Client {
    transport: Arc<Transport>,
    child: Option<Child>,
    events: Mutex<Receiver<Event>>,
    pub capabilities: Capabilities,
}

fn fail(pending: &Pending, failure: &Mutex<Option<String>>, events: &SyncSender<Event>, error: String) {
    let mut state = failure.lock().unwrap();
    if state.is_some() {
        return;
    }
    *state = Some(error.clone());
    for (_, sender) in std::mem::take(&mut *pending.lock().unwrap()) {
        let _ = sender.send(Err(error.clone()));
    }
    let _ = events.try_send(Event::Exited(error));
}

impl Client {
    pub fn start(config: ServerConfig) -> Result<Self, String> {
        if !config.executable.is_absolute() || !config.executable.is_file() {
            return Err(format!("Missing bundled Texlab executable: {}", config.executable.display()));
        }
        let root_uri = uri(&config.root)?;
        let mut child = Command::new(&config.executable)
            .args(&config.arguments)
            .current_dir(&config.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not start bundled Texlab: {error}"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (writer, writes) = mpsc::channel();
        let (events, received) = mpsc::sync_channel(256);
        let pending: Pending = Arc::new(Mutex::new(BTreeMap::new()));
        let failure = Arc::new(Mutex::new(None));
        let transport = Arc::new(Transport {
            writer: writer.clone(),
            pending: Arc::clone(&pending),
            next_id: AtomicU64::new(1),
            failure: Arc::clone(&failure),
        });
        let write_pending = Arc::clone(&pending);
        let write_failure = Arc::clone(&failure);
        let write_events = events.clone();
        thread::spawn(move || {
            let mut stdin = stdin;
            for message in writes {
                if let Err(error) = write_frame(&mut stdin, &message) {
                    fail(&write_pending, &write_failure, &write_events, format!("Texlab write failed: {error}"));
                    break;
                }
            }
        });
        let read_events = events.clone();
        let settings = config.settings.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let message = match read_frame(&mut reader) {
                    Ok(Some(message)) => message,
                    Ok(None) => {
                        fail(&pending, &failure, &read_events, "Texlab exited. Use Restart Language Server.".into());
                        break;
                    }
                    Err(error) => {
                        fail(&pending, &failure, &read_events, format!("Texlab protocol failed: {error}"));
                        break;
                    }
                };
                if let Some(method) = message["method"].as_str() {
                    if let Some(id) = message.get("id") {
                        let response = match method {
                            "workspace/configuration" => {
                                let values: Vec<_> = message["params"]["items"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .map(|item| {
                                        let section = item["section"].as_str().unwrap_or_default();
                                        if section.is_empty() {
                                            return settings.clone();
                                        }
                                        section.split('.').fold(&settings, |value, part| &value[part]).clone()
                                    })
                                    .collect();
                                json!({"jsonrpc":"2.0","id":id,"result":values})
                            }
                            "window/workDoneProgress/create" => json!({"jsonrpc":"2.0","id":id,"result":null}),
                            _ => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported client request"}}),
                        };
                        let _ = writer.send(response);
                    } else {
                        let _ = read_events.try_send(Event::Notification {
                            method: method.into(),
                            params: message["params"].clone(),
                        });
                    }
                } else if let Some(id) = message["id"].as_u64()
                    && let Some(sender) = pending.lock().unwrap().remove(&id)
                {
                    let result = if message.get("error").is_some() {
                        Err(message["error"].to_string())
                    } else {
                        Ok(message["result"].clone())
                    };
                    let _ = sender.send(result);
                }
            }
        });
        thread::spawn(move || {
            let mut stderr = stderr;
            let mut bytes = [0; 4096];
            loop {
                match stderr.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        let _ = events.try_send(Event::Stderr(String::from_utf8_lossy(&bytes[..count]).into()));
                    }
                }
            }
        });
        let mut client = Self {
            transport,
            child: Some(child),
            events: Mutex::new(received),
            capabilities: Capabilities::parse(&json!({}))?,
        };
        let initialized = client
            .request(
                "initialize",
                json!({
                    "processId":std::process::id(),"clientInfo":{"name":"Ligature"},"rootUri":root_uri,
                    "initializationOptions":{"skipDistro":true},
                    "workspaceFolders":[{"uri":root_uri,"name":config.root.file_name().unwrap_or_default().to_string_lossy()}],
                    "capabilities":{
                        "general":{"positionEncodings":["utf-8","utf-16","utf-32"]},
                        "workspace":{"configuration":true},
                        "textDocument":{
                            "synchronization":{"didSave":true},
                            "publishDiagnostics":{"versionSupport":true},
                            "completion":{"completionItem":{"snippetSupport":true,"insertReplaceSupport":true}},
                            "hover":{"contentFormat":["plaintext"]}
                        }
                    }
                }),
            )?
            .wait(Duration::from_secs(10));
        let initialized = initialized.map_err(|error| {
            let stderr: String = client
                .events()
                .into_iter()
                .filter_map(|event| match event {
                    Event::Stderr(text) => Some(text),
                    _ => None,
                })
                .collect();
            if stderr.is_empty() { error } else { format!("{error}\n{stderr}") }
        })?;
        client.capabilities = Capabilities::parse(&initialized["capabilities"])?;
        client.notify("initialized", json!({}))?;
        client.notify("workspace/didChangeConfiguration", json!({"settings":config.settings}))?;
        Ok(client)
    }

    pub fn request(&self, method: &str, params: Value) -> Result<Request, String> {
        self.transport.request(method, params)
    }
    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.transport.notify(method, params)
    }
    pub fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().try_iter().take(256).collect()
    }
    pub fn failure(&self) -> Option<String> {
        self.transport.failure.lock().unwrap().clone()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let transport = Arc::clone(&self.transport);
        thread::spawn(move || {
            if let Ok(mut request) = transport.request("shutdown", Value::Null) {
                let _ = request.wait(Duration::from_secs(1));
            }
            let _ = transport.notify("exit", Value::Null);
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        });
    }
}
