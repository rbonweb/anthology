//! HTTPS fetch over ureq (RT2).
//!
//! The OS trust store (design §11.2) is **not** ureq's default: ureq 3 uses
//! `RootCerts::WebPki` unless the agent's TLS config sets `RootCerts::PlatformVerifier`.
//! RT2 sets it explicitly and tests that a CA known only to the OS store is trusted (DV1 and
//! DV3 do the same when they fetch).
