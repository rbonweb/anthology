# Changelog

All notable changes are recorded here, per plugin. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and each plugin uses
[Semantic Versioning](https://semver.org/) on its own: a release is tagged
`<plugin>--v<version>`, and `scripts/release.sh` moves a plugin's Unreleased entries into
a new version section.

## anthology

### [Unreleased]

#### Added

- Plugin manifest with version-range dependencies on `anthology-guardrails` and
  `anthology-superpowers`, so installing `anthology` brings in both.
- Knowledge library: 25 skill folders (design, engineering, product and marketing),
  generated from pinned upstream sources by `scripts/vendor.py`.
- Routing guide, Memory section: the lead commits the `.claude/agent-memory/` notes that
  read-only and docs-only agents leave uncommitted, and acts on each report's "Worth
  remembering" items.

#### Security

- rtk `1.2.1-anthology.2`: the rtk hook applies your deny, ask and `Read()` deny rules (user,
  project, local and managed settings) to the command an `rtk …` form wraps, printing an
  explicit deny or ask, so `Bash(rtk *)` no longer runs a command one of those rules covers.
- rtk's local history (command lines, recall store, tee spool) is kept in
  `~/.claude/anthology/rtk/` and deleted by `/anthology:setup remove`; README and SECURITY
  document what it stores and for how long.

## anthology-guardrails

### [Unreleased]

#### Added

- Plugin manifest and marketplace entry; the hooks and rules arrive in later releases.

## anthology-superpowers

### [Unreleased]

#### Added

- The upstream superpowers 7.0.2 skills library, renamed to `anthology-superpowers`, kept
  reproducible by `scripts/vendor.py` and versioned independently of upstream.
