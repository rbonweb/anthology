//! Fuzz target `check_repo` (check-repo Task 15): the scanner never panics on any bytes. The same
//! input goes through the three entry points that read text a stranger wrote:
//!
//! - `jsonc::parse`, the comment-tolerant JSON reader behind `.vscode/tasks.json` and friends;
//! - `yara::compile` on the input as rule source, and `eval` of what compiled against the input;
//! - `scan` of a one-file tree on disk whose file holds the input.
//!
//! Input: a first byte that picks the file's name (so every file kind and auto-run surface is
//! reached), then the content. The tree is a fresh temp folder per run; nothing in it is run.
#![no_main]

use std::fs;

use anthology_scan::tree::{DiskTree, WalkOpts};
use anthology_scan::{FileCtx, FileKind, jsonc, scan, yara};

/// Names that route a file to a different rule: auto-run files, agent files, manifests, lockfiles,
/// scripts in every language, data and a binary.
const NAMES: &[&str] = &[
    "package.json",
    ".vscode/tasks.json",
    ".vscode/settings.json",
    ".claude/settings.json",
    ".mcp.json",
    ".devcontainer/devcontainer.json",
    ".devcontainer.json",
    "pyproject.toml",
    ".cargo/config.toml",
    ".codex/config.toml",
    "setup.py",
    "Makefile",
    "configure.m4",
    "package-lock.json",
    "requirements.txt",
    ".gitmodules",
    ".gitattributes",
    ".env",
    ".envrc",
    ".npmrc",
    ".cursorrules",
    "CLAUDE.md",
    "AGENTS.md",
    ".claude/agents/a.md",
    ".github/workflows/ci.yml",
    "gradle/wrapper/gradle-wrapper.properties",
    "app.csproj",
    "index.js",
    "main.py",
    "run.sh",
    "run.ps1",
    "a.bat",
    "lib.rs",
    "font.woff2",
    "image.png",
    "README.md",
    "data.bin",
];

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Some((&pick, content)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(content);

    let _ = jsonc::parse(&text);

    let (rules, _skipped) = yara::compile(&[("fuzz", &text)]);
    let ctx = FileCtx {
        path: "fuzz.js",
        bytes: content,
        text: Some(&text),
        kind: FileKind::Data,
        truncated: false,
    };
    let _ = rules.eval(&ctx);

    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join(NAMES[usize::from(pick) % NAMES.len()]);
    fs::create_dir_all(path.parent().expect("a parent")).expect("create dirs");
    fs::write(&path, content).expect("write the file");
    let tree = DiskTree::open(dir.path(), &WalkOpts::default(), None).expect("open the tree");
    let _ = scan(&tree);
});
