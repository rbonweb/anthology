//! The real ureq download path (RT2) against the testkit HTTP and TLS servers: https only on
//! every hop, the per-read and overall deadlines, the 64 MiB cap (scaled down), the proxy from
//! the `Env` (`HTTPS_PROXY`, `NO_PROXY`), and no partial file after a failure.
//!
//! The servers' certificates come from a test CA that only [`UreqFetch::with_test_roots`]
//! trusts; production uses the platform verifier (unit-tested in `fetch.rs`), and a fetch
//! with the production roots refuses the test server.
#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_pycompat::pyerr::PyExc;
use anthology_runtime::fetch::{self, UreqFetch};
use anthology_testkit::http::{CA_DER, Route, TestServer};
use anthology_testkit::rt_b::ConnectProxy;

fn trusting(env: &Env) -> UreqFetch {
    UreqFetch::new(env).with_test_roots(&[CA_DER])
}

fn get(f: &UreqFetch, url: &str, timeout: f64) -> Result<Vec<u8>, PyExc> {
    let dir = tempfile::tempdir().expect("tmp");
    let out = dir.path().join("out");
    let r = fetch::fetch_to_path(f, url, &out, timeout);
    match r {
        Ok(()) => Ok(std::fs::read(&out).expect("out")),
        Err(e) => {
            assert!(!out.exists(), "a failed download leaves no file");
            Err(e)
        }
    }
}

#[test]
fn downloads_over_tls_with_the_installer_user_agent() {
    let server = TestServer::https();
    let body: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    server.route("/atl.tar.gz", Route::ok(&body));
    let got = get(&trusting(&Env::default()), &server.url("/atl.tar.gz"), 5.0).expect("fetch");
    assert_eq!(got, body);
    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "GET");
    assert_eq!(reqs[0].header("user-agent"), Some(fetch::USER_AGENT));
}

#[test]
fn the_production_roots_do_not_trust_the_test_server() {
    let server = TestServer::https();
    server.route("/x", Route::ok(b"x"));
    let err = get(&UreqFetch::new(&Env::default()), &server.url("/x"), 5.0).unwrap_err();
    assert_eq!(err.type_name(), "URLError", "{}", err.py_str());
}

#[test]
fn an_https_redirect_is_followed() {
    let server = TestServer::https();
    server.route("/a", Route::redirect(&server.url("/b")));
    server.route("/b", Route::redirect("/c"));
    server.route("/c", Route::ok(b"final"));
    let got = get(&trusting(&Env::default()), &server.url("/a"), 5.0).expect("fetch");
    assert_eq!(got, b"final");
    let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(paths, ["/a", "/b", "/c"]);
}

#[test]
fn a_redirect_to_http_is_refused_before_it_is_followed() {
    let tls = TestServer::https();
    let plain = TestServer::http();
    plain.route("/b", Route::ok(b"downgraded"));
    tls.route("/a", Route::redirect(&plain.url("/b")));
    let err = get(&trusting(&Env::default()), &tls.url("/a"), 5.0).unwrap_err();
    assert_eq!(
        err.py_str(),
        "<urlopen error refusing redirect to a non-https URL>"
    );
    assert!(plain.requests().is_empty(), "the http hop was never made");
}

#[test]
fn a_plain_http_url_is_refused_without_a_request() {
    let plain = TestServer::http();
    plain.route("/x", Route::ok(b"x"));
    let err = get(&trusting(&Env::default()), &plain.url("/x"), 5.0).unwrap_err();
    assert_eq!(err.type_name(), "InstallError");
    assert!(plain.requests().is_empty());
}

#[test]
fn a_redirect_loop_stops_as_urllib_does() {
    // urllib: the same target at most MAX_REPEATS times, so the 5th response raises.
    let server = TestServer::https();
    server.route("/loop", Route::redirect(&server.url("/loop")));
    let err = get(&trusting(&Env::default()), &server.url("/loop"), 5.0).unwrap_err();
    assert_eq!(err.type_name(), "HTTPError");
    assert!(err.py_str().starts_with(
        "HTTP Error 302: The HTTP server returned a redirect error that would lead to an \
         infinite loop.\n"
    ));
    assert_eq!(server.requests().len(), fetch::MAX_REPEATS + 1);
}

#[test]
fn a_long_redirect_chain_stops_after_max_redirects() {
    // urllib: at most MAX_REDIRECTS distinct targets; the 11th redirect raises.
    let server = TestServer::https();
    for i in 0..20 {
        server.route(
            &format!("/r{i}"),
            Route::redirect(&server.url(&format!("/r{}", i + 1))),
        );
    }
    let err = get(&trusting(&Env::default()), &server.url("/r0"), 5.0).unwrap_err();
    assert_eq!(err.type_name(), "HTTPError");
    assert_eq!(server.requests().len(), fetch::MAX_REDIRECTS + 1);
    // Ten hops are fine.
    let server = TestServer::https();
    for i in 0..10 {
        server.route(
            &format!("/r{i}"),
            Route::redirect(&server.url(&format!("/r{}", i + 1))),
        );
    }
    server.route("/r10", Route::ok(b"end"));
    let got = get(&trusting(&Env::default()), &server.url("/r0"), 5.0).expect("10 hops");
    assert_eq!(got, b"end");
}

#[test]
fn an_http_error_status_is_an_http_error() {
    let server = TestServer::https();
    let err = get(&trusting(&Env::default()), &server.url("/missing"), 5.0).unwrap_err();
    assert_eq!(err.py_str(), "HTTP Error 404: Not Found");
}

#[test]
fn content_length_over_the_cap_is_refused() {
    let server = TestServer::https();
    server.route("/big", Route::ok(&[7u8; 4096]));
    let f = trusting(&Env::default()).with_max_bytes(1024);
    let err = get(&f, &server.url("/big"), 5.0).unwrap_err();
    assert_eq!(err.py_str(), "download is larger than 1024 bytes");
}

#[test]
fn a_streamed_body_over_the_cap_is_refused_and_removed() {
    let server = TestServer::https();
    server.route("/endless", Route::ok(b"").never_ending());
    let f = trusting(&Env::default()).with_max_bytes(64 * 1024);
    let err = get(&f, &server.url("/endless"), 5.0).unwrap_err();
    assert_eq!(err.py_str(), "download is larger than 65536 bytes");
}

#[test]
fn a_trickling_body_hits_the_overall_deadline() {
    let server = TestServer::https();
    // 1 byte every 50 ms never trips the per-read timeout (0.25 s), only the 4 x 0.25 s total.
    server.route(
        "/slow",
        Route::ok(&[1u8; 100_000]).slow(1, Duration::from_millis(50)),
    );
    let start = Instant::now();
    let err = get(&trusting(&Env::default()), &server.url("/slow"), 0.25).unwrap_err();
    let took = start.elapsed();
    assert!(err.py_str().contains("timed out"), "{}", err.py_str());
    assert!(took < Duration::from_secs(3), "{took:?}");
}

#[test]
fn https_proxy_tunnels_with_connect() {
    let server = TestServer::https();
    server.route("/x", Route::ok(b"via proxy"));
    let proxy = ConnectProxy::start();
    let env = Env::default().with("HTTPS_PROXY", &proxy.url());
    let got = get(&trusting(&env), &server.url("/x"), 5.0).expect("fetch");
    assert_eq!(got, b"via proxy");
    assert_eq!(proxy.targets(), [format!("127.0.0.1:{}", server.port())]);
}

#[test]
fn no_proxy_bypasses_the_proxy() {
    let server = TestServer::https();
    server.route("/x", Route::ok(b"direct"));
    let proxy = ConnectProxy::start();
    let env = Env::default()
        .with("https_proxy", &proxy.url())
        .with("NO_PROXY", "localhost,127.0.0.1");
    let got = get(&trusting(&env), &server.url("/x"), 5.0).expect("fetch");
    assert_eq!(got, b"direct");
    assert!(proxy.targets().is_empty());
}

#[test]
fn a_dead_proxy_fails_without_falling_back_to_direct() {
    let server = TestServer::https();
    server.route("/x", Route::ok(b"x"));
    // A port nothing listens on.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = dead.local_addr().expect("addr").port();
    drop(dead);
    let env = Env::default().with("HTTPS_PROXY", &format!("http://127.0.0.1:{port}"));
    assert!(get(&trusting(&env), &server.url("/x"), 2.0).is_err());
    assert!(server.requests().is_empty());
}
