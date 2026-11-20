//! Ports of tests/test_vendor.py: one module per Python test class, each test keeping the
//! Python method name (plan §0.1.5). The fixture stubs `fetch`, so no test needs the network;
//! the tests Python skips without git return early when git is missing.

mod fixture;

mod rename_plugin_test;
mod rename_skill_test;
mod sources_json_test;
mod strip_connectors_test;
mod url_source_validation_test;
