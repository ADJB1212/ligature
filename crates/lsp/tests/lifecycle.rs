#![cfg(unix)]

use ligature_lsp::{Client, Event, Request, ServerConfig};
use serde_json::{Value, json};
use std::{
    thread,
    time::{Duration, Instant},
};

fn shell_server(root: &std::path::Path, behavior: &str) -> ServerConfig {
    let initialize =
        json!({"jsonrpc":"2.0","id":1,"result":{"capabilities":{"textDocumentSync":1,"completionProvider":{},"hoverProvider":true}}}).to_string();
    let script = format!(
        r#"
read_message() {{
    IFS= read -r header || exit 0
    IFS= read -r blank || exit 0
    count=${{header#*: }}
    count=${{count%?}}
    body=$(dd bs=1 count="$count" 2>/dev/null)
}}
read_message
printf 'Content-Length: {}\r\n\r\n%s' '{}'
{}
"#,
        initialize.len(),
        initialize,
        behavior
    );
    ServerConfig {
        executable: "/bin/sh".into(),
        arguments: vec!["-c".into(), script],
        root: root.into(),
        settings: json!({}),
    }
}

fn response(request: &mut Request) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = request.poll() {
            return result;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn server_exit_fails_pending_requests_and_captures_stderr() {
    let root = tempfile::tempdir().unwrap();
    let config = shell_server(
        root.path(),
        r#"
while true; do
    read_message
    case "$body" in
        *textDocument/completion*) printf 'server crashed' >&2; exit 7 ;;
    esac
done
"#,
    );
    let client = Client::start(config).unwrap();
    let mut request = client.request("textDocument/completion", json!({})).unwrap();
    assert!(response(&mut request).is_err());
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut stderr = String::new();
    while Instant::now() < deadline && !stderr.contains("server crashed") {
        for event in client.events() {
            if let Event::Stderr(text) = event {
                stderr.push_str(&text);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(stderr.contains("server crashed"));
    assert!(client.request("textDocument/hover", json!({})).is_err());
}

#[test]
fn responses_match_ids_out_of_order_and_drop_shuts_down_gracefully() {
    let root = tempfile::tempdir().unwrap();
    let second = json!({"jsonrpc":"2.0","id":3,"result":"hover"}).to_string();
    let first = json!({"jsonrpc":"2.0","id":2,"result":"completion"}).to_string();
    let shutdown = json!({"jsonrpc":"2.0","id":4,"result":null}).to_string();
    let behavior = format!(
        r#"
while true; do
    read_message
    case "$body" in
        *textDocument/hover*) printf 'Content-Length: {}\r\n\r\n%sContent-Length: {}\r\n\r\n%s' '{}' '{}' ;;
        *shutdown*) printf 'Content-Length: {}\r\n\r\n%s' '{}'; printf 'shutdown' > shutdown.txt ;;
        *'"method":"exit"'*) printf 'exit' > exited.txt; exit 0 ;;
    esac
done
"#,
        second.len(),
        first.len(),
        second,
        first,
        shutdown.len(),
        shutdown
    );
    let client = Client::start(shell_server(root.path(), &behavior)).unwrap();
    let mut completion = client.request("textDocument/completion", json!({})).unwrap();
    let mut hover = client.request("textDocument/hover", json!({})).unwrap();
    assert_eq!(response(&mut completion).unwrap(), "completion");
    assert_eq!(response(&mut hover).unwrap(), "hover");
    drop(completion);
    drop(hover);
    drop(client);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !root.path().join("exited.txt").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(root.path().join("shutdown.txt").is_file());
    assert!(root.path().join("exited.txt").is_file());
}

#[test]
fn dropping_an_obsolete_request_sends_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let config = shell_server(
        root.path(),
        r#"
while true; do
    read_message
    case "$body" in
        *cancelRequest*) printf 'canceled' > canceled.txt; exit 0 ;;
    esac
done
"#,
    );
    let client = Client::start(config).unwrap();
    let request = client.request("textDocument/completion", json!({})).unwrap();
    drop(request);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !root.path().join("canceled.txt").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(root.path().join("canceled.txt").is_file());
}

#[test]
fn missing_application_owned_server_reports_a_startup_failure() {
    let root = tempfile::tempdir().unwrap();
    let config = ServerConfig {
        executable: root.path().join("missing-texlab"),
        arguments: Vec::new(),
        root: root.path().into(),
        settings: json!({}),
    };
    assert!(Client::start(config).err().unwrap().contains("Missing bundled Texlab executable"));
}
