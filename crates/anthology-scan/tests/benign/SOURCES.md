# Where the benign samples come from

Each folder here is scanned by `tests/benign.rs` as if it were a repo. The files are copied
unchanged from public projects at the commit named below, one file (or two) per folder, with no
other part of the project. Nothing here is built or run. The license text of each project is in
`LICENSES/`.

| Folder | Project | Commit | License | File in the project | Why it is here |
|---|---|---|---|---|---|
| `vscode-tasks/` | https://github.com/microsoft/vscode | `adf84acb33e978b8c78cb4c9d2f7343766b1d315` | MIT (`LICENSES/vscode-MIT.txt`) | `extensions/copilot/.vscode/tasks.json` | A watch task that runs on folder open, with `presentation.reveal: never` |
| `vscode-devcontainer/` | https://github.com/microsoft/vscode | `adf84acb33e978b8c78cb4c9d2f7343766b1d315` | MIT (`LICENSES/vscode-MIT.txt`) | `extensions/copilot/.devcontainer/devcontainer.json` | A dev container with features and a container-side `onCreateCommand` |
| `husky/` | https://github.com/typicode/husky | `3c0e08d3ca4d01d04ebb92089e68c47e131ab6be` (tag `v8.0.3`) | MIT (`LICENSES/husky-MIT.txt`) | `package.json`, `.husky/pre-commit` | A `prepare` script that builds and installs git hooks |
| `esbuild/` | https://github.com/evanw/esbuild | `f6058f8364fe7ab91ca57a83e02577ed74c9cae4` | MIT (`LICENSES/esbuild-MIT.txt`) | `npm/esbuild/package.json` | A `postinstall` that runs the package's own `install.js` |
| `sharp/` | https://github.com/lovell/sharp | `eefaa998725cf345227d94b40615e090495c6d09` (tag `v0.32.6`) | Apache-2.0 (`LICENSES/sharp-Apache-2.0.txt`) | `package.json` | An `install` chain of node scripts and `prebuild-install` |
| `claude-prettier-hook/` | https://github.com/anthropics/claude-code-action | `4411a9e9b0283becf4b924f7bce360fd7756c78c` | MIT (`LICENSES/claude-code-action-MIT.txt`) | `.claude/settings.json` | A `PostToolUse` hook that runs a pinned `prettier` through `bunx` |
| `mcp-pinned/` | https://github.com/ChilliCream/graphql-platform | `df96118fa451c58aa23f4f47ba6c0be6779434c1` | MIT (`LICENSES/graphql-platform-MIT.txt`) | `.mcp.json` | MCP servers pinned to a version, plus a remote server |
| `ripgrep/` | https://github.com/BurntSushi/ripgrep | `3e85c6c148fdce38a99ceeb9d2780dc7b03b4a4f` | MIT or Unlicense (`LICENSES/ripgrep-MIT.txt`) | `build.rs` | A build script that starts `git rev-parse` |
| `flask/` | https://github.com/pallets/flask | `49943b5a94413a530761350f04e896f76e8d620e` | BSD-3-Clause (`LICENSES/flask-BSD-3-Clause.txt`) | `pyproject.toml` | A `pyproject.toml` with a build backend |
| `gradle-wrapper/` | https://github.com/spring-projects/spring-petclinic | `3aae6ccdd35535054dc972439a461243d41655e2` | Apache-2.0 (`LICENSES/spring-petclinic-Apache-2.0.txt`) | `gradle/wrapper/gradle-wrapper.properties` | A Gradle distribution URL on `services.gradle.org` |
| `minified-bundle/` | https://github.com/zenorocha/clipboard.js | `899378dee9681dcf4cb3d702c23a3f3cd9f473d8` | MIT (`LICENSES/clipboard-js-MIT.txt`) | `dist/clipboard.min.js` | A minified bundle: long lines, no blanks to push code off screen |
| `unicode-text/` | written for this test | none | CC0 | none | Emoji sequences, a flag, a keycap, Persian and Arabic text and a variation selector after a kanji: characters a hidden-text rule must leave alone |

The files were fetched with `git clone --depth 1 --filter=blob:none --no-checkout` into a scratch
folder outside the repo and one `git checkout <commit> -- <file>` per file, then copied in. The
`minified-bundle/` file sits at the top of its folder, not under `dist/`, so the blob rule that
skips `dist/` does not hide it.
