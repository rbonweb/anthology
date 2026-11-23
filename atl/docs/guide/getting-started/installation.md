# Installation

ATL (Anthology Token Layer) is installed by the Anthology plugin. Its session-start hook downloads
the `atl` binary that the plugin pins, checks it against the pinned checksum, and installs it under
the plugin's data folder. You do not install it by hand, and the Anthology setup skill allows
`Bash(atl *)` for it.

This page is for anyone who builds ATL from source or runs it outside Anthology.

## Build from source

ATL is its own Cargo workspace in the `atl/` folder of https://github.com/rbonweb/anthology. It
depends on `../crates/anthology-shell`, so clone the whole repository:

```bash
git clone https://github.com/rbonweb/anthology
cd anthology/atl
cargo build --release
target/release/atl --version
```

## Verify installation

```bash
atl --version   # atl x.y.z
atl gain        # savings dashboard
```

If `atl gain` prints "command not found", ATL is not on your `PATH`.

## Project initialization

`atl init` registers the rewrite hook for an agent outside Anthology. Inside Anthology, the plugin's
own hook does this, so do not run it there.

## Upstream

ATL is a fork of rtk by RTK AI Labs (https://github.com/rtk-ai/rtk). Upstream's own install
methods (Homebrew, winget, its install script) install upstream's legacy `rtk` binary, not ATL.
