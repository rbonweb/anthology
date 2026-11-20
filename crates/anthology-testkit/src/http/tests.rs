use super::*;
use crate::run::{Output, run_program_timeout, which};
use anthology_core::Env;
use std::path::Path;
use std::time::Instant;

fn body_of(resp: &[u8]) -> &[u8] {
    let at = resp
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a head");
    &resp[at + 4..]
}

fn tool_env(server: &TestServer) -> Env {
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    let ca = server.ca_pem_path();
    Env::default()
        .with_os("PATH", path)
        .with("HOME", "/nonexistent")
        .with_os("SSL_CERT_FILE", &ca)
        .with_os("NODE_EXTRA_CA_CERTS", &ca)
}

fn tool(name: &str) -> Option<PathBuf> {
    let found = which(name);
    if found.is_none() {
        eprintln!("skipped: {name} is not installed");
    }
    found
}

fn run(program: &Path, args: &[&str], env: &Env, secs: u64) -> Output {
    run_program_timeout(
        program,
        args,
        b"",
        env,
        Path::new("/"),
        Some(Duration::from_secs(secs)),
    )
}

#[test]
fn plain_http_serves_routes_404s_and_records_requests() {
    let server = TestServer::http();
    server.route("/a", Route::ok(b"alpha").header("X-Test", "1"));
    let resp = raw_get(&server, "/a?x=1", None).expect("get");
    let text = String::from_utf8_lossy(&resp);
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
    assert!(text.contains("X-Test: 1\r\n"), "{text}");
    assert!(text.contains("Content-Length: 5\r\n"), "{text}");
    assert_eq!(body_of(&resp), b"alpha");
    let resp = raw_get(&server, "/missing", None).expect("get");
    assert!(resp.starts_with(b"HTTP/1.1 404 Not Found\r\n"));
    let reqs = server.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].method, "GET");
    assert_eq!(reqs[0].path, "/a?x=1");
    assert_eq!(reqs[0].header("user-agent"), Some("testkit"));
    assert_eq!(reqs[1].path, "/missing");
    assert!(server.url("/a").starts_with("http://127.0.0.1:"));
}

#[test]
fn tls_works_with_the_test_ca_and_fails_without_it() {
    let server = TestServer::https();
    server.route("/f", Route::ok(b"secure"));
    assert!(server.url("/f").starts_with("https://127.0.0.1:"));
    let resp = raw_get(&server, "/f", None).expect("TLS with the test CA");
    assert_eq!(body_of(&resp), b"secure");
    let err = raw_get(&server, "/f", Some(rustls::RootCertStore::empty()))
        .expect_err("an empty root store refuses the server");
    assert!(err.to_string().contains("certificate"), "{err}");
    assert_eq!(server.ca_der(), CA_DER);
    assert_eq!(
        std::fs::read_to_string(server.ca_pem_path()).expect("pem"),
        CA_PEM
    );
}

#[test]
fn slow_mode_drips_the_body() {
    let server = TestServer::http();
    server.route(
        "/s",
        Route::ok(b"0123456789").slow(2, Duration::from_millis(60)),
    );
    let start = Instant::now();
    let resp = raw_get(&server, "/s", None).expect("get");
    assert_eq!(body_of(&resp), b"0123456789");
    assert!(
        start.elapsed() >= Duration::from_millis(5 * 60),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn never_ending_mode_keeps_sending_until_the_client_leaves() {
    let server = TestServer::http();
    server.route(
        "/n",
        Route::ok(b"x").never_ending_with(1000, Duration::from_millis(1)),
    );
    let mut stream = TcpStream::connect(server.addr()).expect("connect");
    stream
        .write_all(b"GET /n HTTP/1.1\r\nHost: x\r\n\r\n")
        .expect("send");
    let mut got = 0usize;
    let mut buf = [0u8; 4096];
    while got < 200_000 {
        let n = stream.read(&mut buf).expect("read");
        assert!(n > 0, "the server stopped after {got} bytes");
        got += n;
    }
    drop(stream);
    let start = Instant::now();
    drop(server);
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn dropping_the_server_ends_a_never_ending_response() {
    // (Whether the port refuses connections afterwards is not checked: a parallel test may
    // be given the same port at once.)
    let server = TestServer::http();
    server.route(
        "/n",
        Route::ok(b"z").never_ending_with(100, Duration::from_millis(1)),
    );
    let mut stream = TcpStream::connect(server.addr()).expect("connect");
    stream
        .write_all(b"GET /n HTTP/1.1\r\nHost: x\r\n\r\n")
        .expect("send");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let mut buf = [0u8; 4096];
    assert!(stream.read(&mut buf).expect("read") > 0);
    let start = Instant::now();
    drop(server);
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => assert!(start.elapsed() < Duration::from_secs(5), "still streaming"),
        }
    }
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn redirects_name_an_absolute_url() {
    let target = TestServer::https();
    target.route("/real", Route::ok(b"moved"));
    let server = TestServer::http();
    server.route("/r", Route::redirect(&target.url("/real")));
    let resp = raw_get(&server, "/r", None).expect("get");
    let text = String::from_utf8_lossy(&resp);
    assert!(text.starts_with("HTTP/1.1 302 Found\r\n"), "{text}");
    assert!(
        text.contains(&format!("Location: {}\r\n", target.url("/real"))),
        "{text}"
    );
}

#[test]
fn curl_follows_a_redirect_to_https_with_the_ca_file() {
    let Some(curl) = tool("curl") else { return };
    let target = TestServer::https();
    target.route("/real", Route::ok(b"via curl"));
    let server = TestServer::http();
    server.route("/r", Route::redirect(&target.url("/real")));
    let env = tool_env(&target);
    let ca = target.ca_pem_path();
    let ca = ca.to_string_lossy();
    let out = run(
        &curl,
        &["-fsSL", "--cacert", &ca, &server.url("/r")],
        &env,
        30,
    );
    assert_eq!(out.code, 0, "{}", out.stderr_text());
    assert_eq!(out.stdout, b"via curl");
    // Without the CA, curl refuses the server (60: the peer certificate cannot be verified).
    let out = run(
        &curl,
        &["-fsS", &target.url("/real")],
        &env.clone().without("SSL_CERT_FILE"),
        30,
    );
    assert_ne!(out.code, 0);
    assert_eq!(
        target.requests().last().map(|r| r.path.as_str()),
        Some("/real")
    );
}

#[test]
fn curl_max_time_ends_a_never_ending_download() {
    let Some(curl) = tool("curl") else { return };
    let server = TestServer::https();
    server.route("/n", Route::ok(b"").never_ending());
    let ca = server.ca_pem_path();
    let ca = ca.to_string_lossy();
    let start = Instant::now();
    let out = run(
        &curl,
        &[
            "-sS",
            "--cacert",
            &ca,
            "--max-time",
            "1",
            "-o",
            "/dev/null",
            &server.url("/n"),
        ],
        &tool_env(&server),
        30,
    );
    assert_eq!(out.code, 28, "{}", out.stderr_text());
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[test]
fn node_trusts_the_server_through_node_extra_ca_certs() {
    let Some(node) = tool("node") else { return };
    let server = TestServer::https();
    server.route("/j", Route::ok(b"{\"ok\": true}"));
    let script = format!(
        "require('https').get({:?}, r => {{ let b = ''; r.on('data', d => b += d); \
         r.on('end', () => {{ process.stdout.write(r.statusCode + ' ' + b); }}); }})\
         .on('error', e => {{ process.stderr.write(String(e)); process.exit(1); }});",
        server.url("/j")
    );
    let out = run(&node, &["-e", &script], &tool_env(&server), 60);
    assert_eq!(out.code, 0, "{}", out.stderr_text());
    assert_eq!(out.stdout_text(), "200 {\"ok\": true}");
}

#[test]
fn busybox_wget_downloads_over_http_and_https() {
    let Some(busybox) = tool("busybox") else {
        return;
    };
    let mut servers = vec![TestServer::http()];
    // BusyBox wget hands https to a helper. Alpine's is `ssl_client`; Debian's busybox (the
    // rust image) tries `openssl s_client` and fails with "Connection reset by peer" before it
    // reaches any server. So https is tested only where `ssl_client` exists; BS1's
    // shims-distros job runs Alpine.
    if which("ssl_client").is_some() {
        servers.push(TestServer::https());
    } else {
        eprintln!("skipped: busybox wget over https (no ssl_client helper)");
    }
    for server in servers {
        server.route("/b", Route::ok(b"busy"));
        let out = run(
            &busybox,
            &["wget", "-q", "-O", "-", &server.url("/b")],
            &tool_env(&server),
            30,
        );
        assert_eq!(out.code, 0, "{}: {}", server.url("/b"), out.stderr_text());
        assert_eq!(out.stdout, b"busy");
    }
}

#[test]
fn many_parallel_clients_are_served() {
    let server = Arc::new(TestServer::https());
    server.route("/p", Route::ok(&[7u8; 100_000]));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let s = Arc::clone(&server);
            thread::spawn(move || raw_get(&s, "/p", None).expect("get"))
        })
        .collect();
    for t in threads {
        let resp = t.join().expect("thread");
        assert_eq!(body_of(&resp).len(), 100_000);
    }
    assert_eq!(server.requests().len(), 8);
}

#[test]
fn head_requests_get_no_body() {
    let server = TestServer::http();
    server.route("/h", Route::ok(b"body"));
    let mut stream = TcpStream::connect(server.addr()).expect("connect");
    stream
        .write_all(b"HEAD /h HTTP/1.1\r\nHost: x\r\n\r\n")
        .expect("send");
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).expect("read");
    assert!(resp.ends_with(b"Content-Length: 4\r\nConnection: close\r\n\r\n"));
    assert_eq!(server.requests()[0].method, "HEAD");
}
