// The `[upstream.*]` headers live in the pipeline engine (`rtk-engine`, BUSL-1.1):
// `src/core/task_cmd.rs` and `src/core/classify.rs` compile only with the
// `engine` feature, so this test does too.
#![cfg(all(unix, feature = "engine"))]
//! Fork change 8 (rbonweb/rtk cc11f851): an `[upstream.*]` `headers` table is sent
//! with every worker call, after rtk's own headers. `{session}` becomes one fresh
//! `rtk-` + 32 hex id per call, shared by every header of that call, and protected
//! names (`Authorization`, `x-api-key`, `x-goog-api-key`, `Content-Type`) are
//! ignored so a config can neither replace nor leak the credential.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

mod common;

const ANSWER: &str = r#"{"choices":[{"message":{"content":"FORK8-ANSWER"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;

/// A request's headers as lower-cased `(name, value)` pairs.
type Headers = Vec<(String, String)>;

/// Accepts one HTTP request, answers it with [`ANSWER`], and returns its headers,
/// or `None` when no request arrives in time (rtk failed before calling), so the
/// test fails instead of hanging.
fn serve_one() -> (u16, std::thread::JoinHandle<Option<Headers>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let handle = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() > deadline {
                        return None;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => panic!("accept: {e}"),
            }
        };
        stream.set_nonblocking(false).expect("blocking stream");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .expect("read timeout");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut headers = Vec::new();
        let mut line = String::new();
        reader.read_line(&mut line).expect("request line");
        loop {
            line.clear();
            reader.read_line(&mut line).expect("header line");
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some((name, value)) = trimmed.split_once(':') {
                headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
        }
        let length = headers
            .iter()
            .find(|(n, _)| n == "content-length")
            .and_then(|(_, v)| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).expect("request body");
        let mut stream = stream;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            ANSWER.len(),
            ANSWER
        )
        .expect("write response");
        Some(headers)
    });
    (port, handle)
}

fn values<'a>(headers: &'a [(String, String)], name: &str) -> Vec<&'a str> {
    headers
        .iter()
        .filter(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
        .collect()
}

#[test]
fn fork_8_configured_headers_reach_the_worker_and_protected_ones_do_not() {
    let (port, server) = serve_one();
    let pipeline = format!(
        r#"[upstream.gateway]
provider    = "openai"
model       = "test-model"
base_url    = "http://127.0.0.1:{port}"
api_key_env = "FORK8_KEY"
headers     = {{ "x-opencode-session" = "{{session}}", "x-trace" = "t-{{session}}", "x-fixed" = "fixed-value", "Authorization" = "Bearer leaked", "X-Api-Key" = "leaked" }}

[task.fork8-read]
kind     = "summarize"
upstream = "gateway"
"#
    );
    let home = common::tempdir();
    // dirs::config_dir() is $XDG_CONFIG_HOME on Linux, ~/Library/Application Support on macOS.
    for dir in [
        home.path().join("rtk"),
        home.path().join("Library/Application Support/rtk"),
    ] {
        std::fs::create_dir_all(&dir).expect("create config dir");
        std::fs::write(dir.join("pipeline.toml"), &pipeline).expect("write pipeline");
    }
    let project = common::tempdir();
    std::fs::write(project.path().join("notes.txt"), "some notes\n").expect("write notes");

    let out = common::rtk_command()
        .args(["fork8-read", "--question", "what is here?", "notes.txt"])
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("FORK8_KEY", "fork8-key")
        .env("RTK_TELEMETRY_DISABLED", "1")
        .output()
        .expect("spawn rtk task");
    assert!(out.status.success(), "{out:?}");
    let headers = server
        .join()
        .expect("server thread")
        .expect("rtk called the configured upstream");

    assert!(
        String::from_utf8_lossy(&out.stdout).contains("FORK8-ANSWER"),
        "{out:?}"
    );

    let session = values(&headers, "x-opencode-session");
    assert_eq!(session.len(), 1, "{headers:?}");
    let id = session[0];
    let hex = id.strip_prefix("rtk-").expect("rtk- prefix");
    assert_eq!(hex.len(), 32, "{id}");
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
    assert_eq!(
        values(&headers, "x-trace"),
        vec![format!("t-{id}").as_str()],
        "one id per call, shared by every header"
    );
    assert_eq!(values(&headers, "x-fixed"), vec!["fixed-value"]);

    assert_eq!(
        values(&headers, "authorization"),
        vec!["Bearer fork8-key"],
        "a configured Authorization never replaces the credential"
    );
    assert!(values(&headers, "x-api-key").is_empty(), "{headers:?}");
}
