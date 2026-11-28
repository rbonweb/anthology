---
name: 'check-repo CLI: the process runner and how URL mode is tested'
description: 'Why session and group kill live in anthology-core sys, why URL tests use a fake ssh, and why a pid check cannot prove a kill in a container'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-cli/src/cmd/check_repo.rs: RealRunner starts each child with env_clear, stdin null and anthology_core::sys::new_session (setsid in pre_exec), and kills the group with sys::kill_group at the deadline. Why there: the workspace allows unsafe only in anthology-core sys.rs (GC-18) and anthology-cli forbids it, so any new libc call goes into sys.rs with a SAFETY comment.
URL mode is tested over ssh://example.invalid/<abs repo path> with a stand-in ssh script first on PATH that runs git upload-pack locally; fetch passes PATH through. An https test server cannot serve a fetch: fetch clears the environment, so git cannot be given the test CA, and the server is only good for the failed-fetch case (certificate error, exit 3, quarantine removed).
In the Rust container nothing reaps orphans, so a killed grandchild stays a zombie and kill -0 still succeeds. Prove a group kill by a pipe reaching EOF (the grandchild holds the write end) or by a file that stops growing.
The scratch folder of the tools is hand-made under TMPDIR (mode 0700, removed on drop) because tempfile is a dev-dependency everywhere; the run refuses when TMPDIR is inside the target.
Tests that write a bidi character into a name see it as an uppercase escape: \u{202E}, not \u{202e}.
