---
name: reference-project-map
description: How the pieces of this project connect, and the cleanups each kind of change brings. Read before planning or briefing.
metadata:
  type: reference
---

<!-- Size budget: 7000 characters for the whole file. Keep one line per piece. Link deeper notes with [[note-name]] instead of copying them. Never put secrets, home paths, emails or private hosts here: everyone with the repo sees it. Delete this comment when you fill in the template. -->

## Overview
Two or three sentences: what the project does, how it is built, and how it is run.

## Pieces
One line per module or piece: `path` - what it does - what it connects to (callers, data, config, tests, docs).

## Flows
The paths that cross several pieces (a request, a data flow, the build, the release), each as a short chain of pieces.

## Checks
Where the tests, linters and CI jobs live, which pieces each one covers, and the one command that runs each.

## Cleanup duties
One line per kind of change, naming what else it must update or remove. Example kinds: add a piece, remove a piece, rename or move a piece, change a public interface, change config or CI, add a dependency, run containers or create worktrees.
