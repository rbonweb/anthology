//! `anthology-dev vendor`: the vendored trees (scripts/vendor.py). Owner: DV1.
//!
//! Every vendored tree is generated from a pinned upstream: pristine → transforms → patches →
//! licenses → target. The functions keep vendor.py's names, arguments and messages, so the
//! Python tests port one to one and `vendor check` reports what `vendor.py check` reports.
//!
//! - [`manifest`]: `vendor/sources.json` (load, save, validate).
//! - [`transforms`]: `rename-plugin`, `strip-connectors`, `rename-skill`.
//! - [`compare`]: copying, snapshotting and comparing trees.
//! - [`py`]: the Python semantics vendor.py relies on.

pub mod build;
pub mod cli;
pub mod compare;
pub mod diff;
pub mod fetch;
pub mod licenses;
pub mod manifest;
pub mod patch;
pub mod portmap;
pub mod py;
pub mod redirect;
pub mod set_atl_upstream;
pub mod sync;
pub mod transforms;

use std::ffi::OsString;
use std::fmt;
use std::io::Read;
use std::path::PathBuf;

use anthology_pycompat::json;

use crate::Output;

pub use build::{BuildStats, build, build_tree, install_tree};
pub use compare::{Snap, check, compare_trees, copy_tree, snapshot};
pub use diff::{diff_entry, diff_text};
pub use fetch::{
    HttpOpts, dispatch_fetcher, download_snapshot, fetch_entry, fetcher, git_fetcher,
    git_resolve_ref, http_get, http_get_with, parse_checksums, snapshot_fetcher, verify_snapshot,
};
pub use licenses::{check_licenses, fetch_licenses, ship_licenses};
pub use manifest::{
    ALLOWED_LICENSES, CACHE_DIRNAME, CHECKSUMS_NAME, FIXED_LICENSE_NAME, IGNORED_EXTRAS,
    KNOWLEDGE_DIR, LICENSES, MAX_DOWNLOAD, NOTICE_NAMES, PORTMAP_DIR, SNAPSHOTS, SOURCES, SUBTREE,
    VENDORED_PLUGIN, find_entry, find_entry_index, is_subtree, load_sources, require_valid,
    safe_portmap, safe_relpath, safe_target, save_sources, source_slug, validate_sources, vendored,
};
pub use patch::apply_patch;
pub use portmap::{load_portmap, portmap_problems, portmap_scripts};
pub use sync::{Resolve, SyncReport, sync};
pub use transforms::{
    TRANSFORMS, rename_plugin, rename_skill, run_transform, strip_connectors, strip_markdown,
};

/// vendor.py's `VendorError`: the message is Python's, verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VendorError(pub String);

impl VendorError {
    pub fn new(msg: impl Into<String>) -> VendorError {
        VendorError(msg.into())
    }
}

impl fmt::Display for VendorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for VendorError {}

pub type Result<T> = std::result::Result<T, VendorError>;

/// One `vendor/sources.json` entry, as loosely typed as the Python dict.
pub type Entry = json::Map;

/// `fetch(entry) -> Path`: a pristine checkout of the entry's upstream.
pub type Fetch<'a> = dyn Fn(&Entry) -> Result<PathBuf> + 'a;

/// `get(url) -> bytes`: one download.
pub type Get<'a> = dyn Fn(&str) -> Result<Vec<u8>> + 'a;

/// Runs `anthology-dev vendor …`; `args` are the arguments after `vendor`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    cli::main(args)
}
