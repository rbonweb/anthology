---
name: 'Slop gate Task 7: CLI hook and check rulings'
description: 'How anthology slop hook reads paths and fails open, and how check resolves PATHs, parses git diff and prints lines'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-cli/src/cmd/slop.rs. The hook normalises file_path lexically (dot and dot-dot) after joining a relative one to the payload cwd, before repo_root and exempt, because exempt() reads components and .claude/../docs/a.md would otherwise be exempt. A relative path with no absolute cwd, a read error, non-UTF-8 text, or a before or after over MAX_DOC all allow. read_doc stats with fs::metadata (follows links) and refuses anything but a regular file, then reads at most MAX_DOC + 1 bytes: a FIFO open blocks forever and a link to /dev/zero never ends, and a hung hook blocks through onFailure. The check walk skips links and non-files; PATHs named directly go through the same read_doc. replace_all may be missing or null; any other non-bool allows. The whole hook runs in catch_unwind with a silent panic hook; release keeps panic = unwind (workspace Cargo.toml), which this relies on.
check: --since with PATHs is a usage error, and a ref that is empty or starts with a dash is refused (it is passed to git merge-base as an argument). Relative PATHs resolve against --root, else the current folder; a missing PATH exits 2. Shown paths are relative to the file own repo root, else to --root. The git diff call adds --no-textconv, --inter-hunk-context=0 and --src-prefix=a/ --dst-prefix=b/ beyond the plan list, because diff.interHunkContext, diff.noprefix and diff.mnemonicPrefix in a user config otherwise skew the added lines or the +++ parse; a test sets those configs. Hunk bodies are counted out from the @@ counts so an added line starting with ++ is never read as a file header; C-quoted names and the trailing tab git adds after a name with a space are handled. Every stdout line and the stderr error line escape control characters, since excerpts and waiver reasons are file text.
Tests (crates/anthology-cli/tests/slop.rs) run the binary with a 30 s deadline and drain its pipes on threads, so a hang fails one test instead of the suite; they set GIT_CEILING_DIRECTORIES to the sandbox so git never finds a repo above it.
Why: these decisions are not in the plan text, and Task 8, Task 10 and later fixes build on them.
