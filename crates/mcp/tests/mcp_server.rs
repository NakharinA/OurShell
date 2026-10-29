//! End-to-end test: a fake agent answers the OurShell agent protocol over a local
//! socket, and the real `ourshell-mcp` binary is driven as a child process over its
//! stdio MCP transport (initialize -> initialized -> tools/list -> tools/call).

use std::process::Stdio;
use std::time::Duration;

use interprocess::local_socket::tokio::{prelude::*, Stream};
use interprocess::local_socket::ListenerOptions;
use ourshell_proto::agent::{method, AiHost, DirEntry, ExecResult, FileData, HelloResult};
use ourshell_proto::rpc::{self, Request, Response, RpcError};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

const TIMEOUT: Duration = Duration::from_secs(10);

/// Answers `hello` and the `ai.*` methods used by the server with canned responses,
/// including one approval-rejected error (-32004) for `ai.write_file`.
fn fake_response(req: &Request) -> Response {
    match req.method.as_str() {
        method::HELLO => Response::ok(
            req.id,
            serde_json::to_value(HelloResult {
                protocol: ourshell_proto::AGENT_PROTOCOL_VERSION,
                agent_version: "fake-agent".into(),
            })
            .unwrap(),
        ),
        method::AI_HOSTS => Response::ok(
            req.id,
            serde_json::to_value(vec![AiHost {
                name: "web".into(),
                tags: vec!["prod".into()],
            }])
            .unwrap(),
        ),
        method::AI_RUN => Response::ok(
            req.id,
            serde_json::to_value(ExecResult {
                stdout: "hello\n".into(),
                stderr: String::new(),
                exit_code: Some(0),
                truncated: false,
            })
            .unwrap(),
        ),
        method::AI_LIST_DIR => Response::ok(
            req.id,
            serde_json::to_value(vec![DirEntry {
                name: "file.txt".into(),
                is_dir: false,
                size: Some(3),
                modified: None,
            }])
            .unwrap(),
        ),
        method::AI_READ_FILE => Response::ok(
            req.id,
            serde_json::to_value(FileData {
                data: b"hi".to_vec(),
                truncated: false,
            })
            .unwrap(),
        ),
        method::AI_WRITE_FILE => Response::err(
            req.id,
            RpcError::new(rpc::APPROVAL_REJECTED, "user said no"),
        ),
        method::AI_K8S_GET
        | method::AI_K8S_DESCRIBE
        | method::AI_K8S_LOGS
        | method::AI_K8S_EVENTS
        | method::AI_SYSTEM_STATUS => Response::ok(
            req.id,
            serde_json::to_value(ExecResult {
                stdout: format!("ok: {}\n", req.method),
                stderr: String::new(),
                exit_code: Some(0),
                truncated: false,
            })
            .unwrap(),
        ),
        other => Response::err(
            req.id,
            RpcError::new(rpc::METHOD_NOT_FOUND, format!("unexpected method {other}")),
        ),
    }
}

async fn handle_fake_conn(stream: Stream) {
    let (r, mut w) = tokio::io::split(stream);
    let mut lines = BufReader::new(r).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Request>(&line) else {
            continue;
        };
        let resp = fake_response(&req);
        let mut out = serde_json::to_vec(&resp).expect("serialize response");
        out.push(b'\n');
        if w.write_all(&out).await.is_err() {
            break;
        }
        let _ = w.flush().await;
    }
}

/// Starts the fake agent listening on a unique socket id, returning that id.
async fn start_fake_agent() -> String {
    let socket_id = format!("ourshell-mcp-test-{}", uuid::Uuid::new_v4());
    let name = ourshell::ipc::socket_name(&socket_id).expect("socket name");
    let listener = ListenerOptions::new()
        .name(name)
        .create_tokio()
        .expect("fake agent listener");
    tokio::spawn(async move {
        while let Ok(conn) = listener.accept().await {
            tokio::spawn(handle_fake_conn(conn));
        }
    });
    socket_id
}

fn spawn_mcp_server(socket_id: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_ourshell-mcp"))
        .env(ourshell::ipc::SOCKET_ENV, socket_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn ourshell-mcp")
}

async fn send<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, msg: Value) {
    let mut line = serde_json::to_vec(&msg).expect("serialize mcp message");
    line.push(b'\n');
    tokio::time::timeout(TIMEOUT, async {
        w.write_all(&line).await.expect("write to mcp server");
        w.flush().await.expect("flush mcp server stdin");
    })
    .await
    .expect("write did not time out");
}

async fn recv<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> Value {
    let mut line = String::new();
    tokio::time::timeout(TIMEOUT, r.read_line(&mut line))
        .await
        .expect("read did not time out")
        .expect("read line from mcp server");
    assert!(!line.trim().is_empty(), "mcp server closed its stdout");
    serde_json::from_str(line.trim()).expect("mcp server wrote valid JSON")
}

fn call_tool(id: u64, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
}

fn tool_text(resp: &Value) -> String {
    resp["result"]["content"]
        .as_array()
        .expect("content array")
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("")
}

#[tokio::test]
async fn lists_tools_and_forwards_calls_to_the_agent() {
    let socket_id = start_fake_agent().await;
    let mut child = spawn_mcp_server(&socket_id);
    let mut stdin = child.stdin.take().expect("mcp stdin");
    let stdout = child.stdout.take().expect("mcp stdout");
    let mut reader = BufReader::new(stdout);

    // --- initialize ---
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "0.0.0" }
            }
        }),
    )
    .await;
    let init_resp = recv(&mut reader).await;
    assert_eq!(init_resp["id"], 1);
    assert!(init_resp["result"]["serverInfo"]["name"] == "ourshell");
    assert!(init_resp["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("prompt injection"));

    send(
        &mut stdin,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;

    // --- tools/list ---
    send(
        &mut stdin,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    let list_resp = recv(&mut reader).await;
    let tools = list_resp["result"]["tools"]
        .as_array()
        .expect("tools array");
    let mut names: Vec<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().expect("tool name"))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "k8s_describe",
            "k8s_events",
            "k8s_get",
            "k8s_logs",
            "list_dir",
            "list_hosts",
            "read_file",
            "run_command",
            "system_status",
            "write_file",
        ]
    );
    for tool in tools {
        assert_eq!(
            tool["inputSchema"]["type"], "object",
            "tool {} should have an object input schema",
            tool["name"]
        );
    }
    let run_command = tools
        .iter()
        .find(|t| t["name"] == "run_command")
        .expect("run_command tool");
    let props = &run_command["inputSchema"]["properties"];
    assert!(props["host"].is_object());
    assert!(props["command"].is_object());

    // --- tools/call: list_hosts ---
    send(&mut stdin, call_tool(3, "list_hosts", json!({}))).await;
    let resp = recv(&mut reader).await;
    assert_eq!(resp["result"]["isError"], false);
    assert!(tool_text(&resp).contains("web [prod]"));

    // --- tools/call: run_command ---
    send(
        &mut stdin,
        call_tool(
            4,
            "run_command",
            json!({"host": "web", "command": "echo hi"}),
        ),
    )
    .await;
    let resp = recv(&mut reader).await;
    assert_eq!(resp["result"]["isError"], false);
    assert!(tool_text(&resp).contains("exit code: 0"));
    assert!(tool_text(&resp).contains("hello"));

    // --- tools/call: list_dir ---
    send(
        &mut stdin,
        call_tool(5, "list_dir", json!({"host": "web", "path": "/tmp"})),
    )
    .await;
    let resp = recv(&mut reader).await;
    assert_eq!(resp["result"]["isError"], false);
    assert!(tool_text(&resp).contains("file.txt"));

    // --- tools/call: read_file ---
    send(
        &mut stdin,
        call_tool(
            6,
            "read_file",
            json!({"host": "web", "path": "/tmp/file.txt"}),
        ),
    )
    .await;
    let resp = recv(&mut reader).await;
    assert_eq!(resp["result"]["isError"], false);
    assert_eq!(tool_text(&resp), "hi");

    // --- tools/call: k8s_get ---
    send(
        &mut stdin,
        call_tool(
            7,
            "k8s_get",
            json!({"host": "web", "kind": "pods", "namespace": "default"}),
        ),
    )
    .await;
    let resp = recv(&mut reader).await;
    assert_eq!(resp["result"]["isError"], false);
    assert!(tool_text(&resp).contains("ai.k8s_get"));

    // --- tools/call: system_status (no arguments) ---
    send(
        &mut stdin,
        call_tool(8, "system_status", json!({"host": "web"})),
    )
    .await;
    let resp = recv(&mut reader).await;
    assert_eq!(resp["result"]["isError"], false);
    assert!(tool_text(&resp).contains("ai.system_status"));

    // --- tools/call: write_file -> the fake agent rejects it (-32004) ---
    send(
        &mut stdin,
        call_tool(
            9,
            "write_file",
            json!({"host": "web", "path": "/tmp/file.txt", "content": "data"}),
        ),
    )
    .await;
    let resp = recv(&mut reader).await;
    assert_eq!(resp["result"]["isError"], true);
    assert_eq!(tool_text(&resp), "the user rejected this action");

    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    if child.id().is_some() {
        let _ = child.kill().await;
    }
}
