---
name: 'anthology-mem --body-file reviewed in PR 156: path policy and read cap'
description: 'Probed gaps in read_body_file: FIFO hang, CRLF truncation at the read cap, any readable file accepted'
metadata:
  type: project
  modified: 2026-11-22
---

Probed on head 80bae534 in the pinned rust image. read_body_file opens before it checks is_file, so a FIFO hangs at open. It reads cap = MAX_BODY_BYTES + 1 raw bytes, so a CRLF file whose normalised size fits (raw about 16.5 KB) is silently truncated and saved with exit 0. Any readable non-secret-shaped file (dotenv, known_hosts, /etc/passwd via symlink) is saved into the repo note; Bash(anthology-mem save *) is auto-allowed, so this reads past Read deny rules. DEFAULT_BUDGET also caps personal-memory and portable-import indexes, not only agent indexes. Mem golden diff was only seed:7 (save usage text, D-1 mark dropped).
Why: record what was verified so a re-review of the fix can skip the probes.
How to apply: after a fix, re-probe with a FIFO, a 500-line CRLF file with a tail marker, and a symlink to a file outside the repo.
