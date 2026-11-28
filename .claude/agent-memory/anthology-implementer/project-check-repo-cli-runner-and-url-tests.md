---
name: 'check-repo CLI: the process runner, cleanup on interrupt and how URL mode is tested'
description: 'Why session and group kill live in anthology-core sys, how the watchdog and signal thread clean up, why URL tests use a fake ssh, and why a pid check cannot prove a kill in a container'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-cli/src/cmd/check_repo.rs: RealRunner starts each child with env_clear, stdin null and anthology_core::sys::new_session (setsid in pre_exec), and kills the group with sys::kill_group at the deadline. Why there: the workspace allows unsafe only in anthology-core sys.rs (GC-18) and anthology-cli forbids it, so any new libc call (new_session, kill_group, catch_termination) goes into sys.rs with a SAFETY comment.
Cleanup: a Guard lists the running child groups and the folders this run made (quarantine, its .<name>.template sibling, scratch). sys::catch_termination turns SIGINT, SIGTERM and SIGHUP into bytes on a self-pipe; a thread calls Guard::shutdown (kill groups, remove folders, refuse new children) and exits 128 plus the signal. Each child also gets a watchdog: /bin/sh in its own session reading a pipe that only this process writes to; a done line means stand down, a closed pipe with no line (SIGKILL, crash) means kill the group and rm -rf the registered folders. macOS has no PDEATHSIG, so the pipe does it. Only a folder that did not exist before the run is registered, because the watchdog runs rm -rf on the list, and every path is made absolute because the watchdog runs in /.
Tests: a hung fake ssh or tool writing a heartbeat file in a loop; the file stops growing once the group is dead. Mutation check: removing the done line semantics made the watchdog delete the quarantine after every git step.
URL mode is tested over ssh://example.invalid/<abs repo path> with a stand-in ssh script first on PATH that runs git upload-pack locally; fetch passes PATH through (absolute entries only). An https test server cannot serve a fetch: fetch clears the environment, so git cannot be given the test CA, and the server is only good for the failed-fetch case (certificate error, exit 3, quarantine removed).
In the Rust container nothing reaps orphans, so a killed grandchild stays a zombie and kill -0 still succeeds. Prove a group kill by a pipe reaching EOF (the grandchild holds the write end) or by a file that stops growing.
The scratch folder of the tools is hand-made under TMPDIR (mode 0700, removed on drop) because tempfile is a dev-dependency everywhere; the run refuses when TMPDIR is inside the target.
Tests that write a bidi character into a name see it as an uppercase escape: \u{202E}, not \u{202e}. A libtest filter after -- applies to every test target of the run, so integration tests need their own run.
