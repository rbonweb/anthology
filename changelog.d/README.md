# Changelog fragments

A pull request that changes what users get adds a fragment here instead of editing `CHANGELOG.md`. Two pull requests then no longer fight over the same lines of that file.

## Where a fragment goes

`changelog.d/<plugin>/<name>.md`, where `<plugin>` is the plugin folder name (`anthology`, `anthology-guardrails`, `anthology-superpowers`) and `<name>` has no spaces. Only `*.md` files directly inside that folder count; hidden files and other extensions are ignored.

## What a fragment holds

The same text you would write under `### [Unreleased]` in `CHANGELOG.md`: one or more `- ` entries, optionally under a `#### Added`, `#### Changed` or `#### Fixed` heading.

```markdown
- `scripts/release.sh` ends with `RELEASE OK <tag>` or `RELEASE FAILED: <step>`.
```

## How the release uses them

`scripts/release.sh <plugin> <version>` builds the new `### [<version>]` section from the plugin's `### [Unreleased]` section followed by its fragments in filename order. Name files so that this order reads well (`10-added-x.md`, `20-fixed-y.md`). A fragment that starts with a heading begins a new block; any other fragment continues the lines above it.

The release commit deletes the fragments it used. The release check accepts a plugin whose `[Unreleased]` section is empty when it has at least one fragment with an entry. `CHANGELOG.md` must still have the plugin's `## <plugin>` section and its `### [Unreleased]` heading.
