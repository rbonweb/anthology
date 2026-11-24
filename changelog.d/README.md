# Changelog fragments

A pull request that changes what users get adds a fragment here instead of editing `CHANGELOG.md`. Two pull requests then no longer fight over the same lines of that file (timing probe).

## Where a fragment goes

`changelog.d/<plugin>/<name>.md`, where `<plugin>` is the plugin folder name (`anthology`, `anthology-guardrails`, `anthology-superpowers`) and `<name>` has no spaces. Only `*.md` files directly inside that folder count; hidden files and other extensions are ignored.

## What a fragment holds

One or more plain bullet entries (lines that start with a dash). Do not write `####` headings (`#### Added` and the like): the release does not merge headings, so two fragments with the same heading would repeat it. Put the kind of change in the entry's first words instead (`Fix:`, `Add:`).

```markdown
- `scripts/release.sh` ends with `RELEASE OK <tag>` or `RELEASE FAILED: <step>`.
```

## How the release uses them

`scripts/release.sh <plugin> <version>` builds the new `### [<version>]` section from the plugin's `### [Unreleased]` section followed by its fragments in filename order. Name files so that this order reads well (`10-x.md`, `20-y.md`). Each fragment's entries continue the lines above them, so they join the list under the last `####` heading of `[Unreleased]`, if it has one.

The release commit deletes the fragments it used. The release check accepts a plugin whose `[Unreleased]` section is empty when it has at least one fragment with an entry. `CHANGELOG.md` must still have the plugin's `## <plugin>` section and its `### [Unreleased]` heading.
