//! Ports of tests/test_vendor.py: one module per Python test class, each test keeping the
//! Python method name (plan §0.1.5). The fixture stubs `fetch`, so no test needs the network;
//! the tests Python skips without git return early when git is missing.

mod compatibility_test;
mod fixture;
mod http_get_test;

mod license_test;
mod pipeline_test;
mod portmap_test;
mod rename_plugin_test;
mod rename_skill_test;
mod repo_api_test;
mod set_atl_upstream_test;
mod ship_license_test;
mod sources_json_test;
mod strip_connectors_test;
mod sync_and_diff_test;
mod tree_test;
mod url_download_test;
mod url_source_test;
mod url_source_validation_test;
