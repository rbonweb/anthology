//! `http_get` against the testkit HTTPS server (no Python counterpart: test_vendor.py only
//! checks the scheme guard). The tests trust the testkit CA through `HttpOpts.roots`, the
//! test-only seam; production keeps `RootCerts::PlatformVerifier`.

use std::time::Duration;

use anthology_dev::vendor::{self, HttpOpts, MAX_DOWNLOAD};
use anthology_testkit::http::{Route, TestServer};

fn opts(server: &TestServer) -> HttpOpts {
    HttpOpts {
        roots: vec![server.ca_der()],
        timeout: Some(Duration::from_secs(10)),
    }
}

fn err(r: vendor::Result<Vec<u8>>) -> String {
    r.unwrap_err().0
}

#[test]
fn http_get_returns_the_body_and_names_itself() {
    let server = TestServer::https();
    server.route("/a", Route::ok(b"hello"));
    assert_eq!(
        vendor::http_get_with(&server.url("/a"), &opts(&server)).unwrap(),
        b"hello"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("user-agent"), Some("anthology-vendor"));
}

#[test]
fn http_get_reports_an_http_error_like_urllib() {
    let server = TestServer::https();
    let url = server.url("/missing");
    assert_eq!(
        err(vendor::http_get_with(&url, &opts(&server))),
        format!("cannot download {url}: HTTP Error 404: Not Found")
    );
}

#[test]
fn http_get_follows_an_https_redirect() {
    let server = TestServer::https();
    server.route("/from", Route::redirect(&server.url("/to")));
    server.route("/to", Route::ok(b"moved"));
    assert_eq!(
        vendor::http_get_with(&server.url("/from"), &opts(&server)).unwrap(),
        b"moved"
    );
}

#[test]
fn http_get_refuses_a_redirect_to_http() {
    let server = TestServer::https();
    let plain = TestServer::http();
    plain.route("/to", Route::ok(b"plain"));
    server.route("/from", Route::redirect(&plain.url("/to")));
    assert_eq!(
        err(vendor::http_get_with(&server.url("/from"), &opts(&server))),
        format!(
            "refusing a redirect to a non-https URL: {}",
            plain.url("/to")
        )
    );
    assert!(plain.requests().is_empty());
}

#[test]
fn http_get_stops_at_the_size_cap() {
    let server = TestServer::https();
    server.route(
        "/big",
        Route::ok(b"").never_ending_with(1 << 20, Duration::ZERO),
    );
    let url = server.url("/big");
    assert_eq!(
        err(vendor::http_get_with(&url, &opts(&server))),
        format!("{url} is larger than {MAX_DOWNLOAD} bytes")
    );
}

#[test]
fn http_get_does_not_trust_the_test_ca_by_default() {
    let server = TestServer::https();
    server.route("/a", Route::ok(b"hello"));
    let url = server.url("/a");
    let e = err(vendor::http_get(&url));
    assert!(e.starts_with(&format!("cannot download {url}: ")), "{e}");
}
