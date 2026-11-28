---
name: 'anthology-scan tree walk and orchestrator: design facts'
description: 'DiskTree read budget, BFS order, octal git modes, hook seam in scan.rs, what the other scan tasks can rely on'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-scan/src/tree.rs: DiskTree walks breadth first with names sorted, so shallow files come first when max_files cuts the walk. read() opens only regular-file entries, checks lstat then fstat (same dev and inode), and adds up bytes read; past max_bytes every read returns ErrorKind::QuotaExceeded and partial() turns true. skipped() holds raw paths with a trailing slash and errors() quote names with the Debug format; the report must escape both. Git modes passed to open are octal values (0o120000 symlink, 0o160000 submodule), as fetch reads them from ls-files; the decimal digits 120000 are not a symlink (lead ruling, pinned by a test). A top-level .git folder gives only .git/config and non-sample .git/hooks files in path mode and nothing when modes are given. src/scan.rs: Hooks is a table of function pointers (REAL = the rule modules); tests swap in stubs so the orchestrator is tested without Tasks 3, 4, 5, 9 and 10. raise_twins is called once with every entry path after the per-file rules (REAL points at a no-op until hidden::raise_homoglyph_twins from Task 4 merges). Chain order: yara hits are mapped (high MEDIUM, medium LOW, low INFO) and raised one level in reached files, then twins are raised, then each Surface is classified and its finding emitted, then one finding per (rule, path) is kept with count. files::magic names are ELF, Mach-O, PE, WebAssembly, zip, gzip, xz, 7z. Why: later tasks (rules, report, CLI) rely on these names and on this error and partial behavior.
