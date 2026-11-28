# Where the benign samples come from

Each folder here is scanned by `tests/benign.rs` as if it were a repo. The files are copied
from public projects at the commit named below, one file (or two) per folder, with no other part
of the project. Nothing here is built or run. The license text of each project is in `LICENSES/`.

Four files are trimmed so they carry no personal contact data: `husky/package.json` and
`flask/pyproject.toml` lose their author and maintainer lines, `sharp/package.json` is trimmed to
its scripts and dependency fields (no author, contributors or integrity strings), and
`esbuild/package.json` loses its binary hash table and most optional platform packages. Every
field a rule reads (the scripts) is unchanged. The other files are copied as they are.

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

Path-mode samples. Git does not store a folder named `.git`, so each of these keeps its git folder
as `dot-git/`; `tests/benign.rs` copies the sample to a temp folder and renames it to `.git` before
the scan, the way a folder the user named looks on disk.

| Folder | Project | Commit | License | What is real | What is written for this test |
|---|---|---|---|---|---|
| `path-husky/` | https://github.com/typicode/husky | `3c0e08d3ca4d01d04ebb92089e68c47e131ab6be` (tag `v8.0.3`) | MIT (`LICENSES/husky-MIT.txt`) | `.husky/pre-commit`, and `husky.sh` as `.husky/_/husky.sh` | `dot-git/config` (the `hooksPath = .husky` line that `husky install` writes), `.husky/_/.gitignore`, `package.json` |
| `path-pre-commit/` | https://github.com/pre-commit/pre-commit | `ba5c4209a6d561b0dff5db24620778d2133d068e` | MIT (`LICENSES/pre-commit-MIT.txt`) | `pre_commit/resources/hook-tmpl` as `dot-git/hooks/pre-commit`, with its two templated lines filled in as `pre-commit install` fills them | `dot-git/config`, `.pre-commit-config.yaml` |
| `path-git-lfs/` | https://github.com/git-lfs/git-lfs | `ac128b57bca49e04041b2ce064a09ac6b1d24d51` | MIT (`LICENSES/git-lfs-MIT.txt`) | the hook text of `hookBaseContent` in `lfs/hook.go`, with the command filled in for `pre-push` and `post-checkout` as `git lfs install` writes it | `dot-git/config`, `.gitattributes` |
| `path-sccache/` | https://github.com/Mesh-LLM/mesh-llm | `74c8765c95b737e1f00cea67c021fa0720452195` | Apache-2.0 (`LICENSES/mesh-llm-Apache-2.0.txt`) | `.cargo/config.toml`, with `rustc-wrapper = "sccache"` and in-repo linker scripts | none |

Samples for the false alarms found in the second review.

| Folder | Project | Commit | License | File in the project | Why it is here |
|---|---|---|---|---|---|
| `vscode-program-keys/` | https://github.com/CaptorAB/openseries | `9f12f7de9dcff7337066ed57163105d21bc3e8d3` | BSD-3-Clause (`LICENSES/openseries-BSD-3-Clause.txt`) | `.vscode/settings.json` | `python.analysis.stubPath`, `python.defaultInterpreterPath` and `ruff.configuration` point into the repo and are not programs the editor runs (the `pyproject.toml` beside it is two lines written for this test) |
| `vscode-prettier-config-path/` | https://github.com/react-hook-form/resolvers | `5e9051ee0eb45534cf257c75c906bbe583c2d1cd` | MIT (`LICENSES/react-hook-form-resolvers-MIT.txt`) | `.vscode/settings.json` | `prettier.configPath` names a config file, and `typescript.tsdk` names a folder under `node_modules` |
| `makefile-golangci/` | https://github.com/google/cadvisor | `cacca250d06e88212b7ac5aa8198d1be064e0ab3` | Apache-2.0 (`LICENSES/cadvisor-Apache-2.0.txt`) | `Makefile` | A lint target that installs `golangci-lint` with `curl ... \| sh`: HIGH, not CRITICAL, because it runs in a build step |
| `vite-ngrok-host/` | https://github.com/instructa/constructa-starter | `17b006e00e1bbc2929d4e53bb0dc8d8e45f244b4` | MIT (`LICENSES/constructa-starter-MIT.txt`) | `vite.config.ts` | An `allowedHosts` entry on an ngrok host, and an `Object.assign(process.env, loadEnv(...))` that reads the environment in, not out |
| `path-husky-v4/` | https://github.com/typicode/husky | `37b6925e303c7da71f28f4f4e2bf33d90dd19d73` (tag `v4.3.8`) | MIT (`LICENSES/husky-MIT.txt`) | the hook, `husky.sh` and `husky.local.sh` that husky 4 writes, taken from its `scripts.ts` snapshot in `src/installer/__tests__/__snapshots__` with the escapes removed and a date filled in | `package.json` and `dot-git/config` are written for this test |

Two more cases are built in `tests/benign.rs` and have no folder here: a Python checkout whose
`.venv`, `venv`, `.tox`, `.nox` or `.direnv` holds an absolute symlink to the interpreter, and a
`Makefile` that is Latin-1.

The files were fetched with `git clone --depth 1 --filter=blob:none --no-checkout` into a scratch
folder outside the repo and one `git checkout <commit> -- <file>` per file, then copied in. The
`minified-bundle/` file sits at the top of its folder, not under `dist/`, so the blob rule that
skips `dist/` does not hide it.
