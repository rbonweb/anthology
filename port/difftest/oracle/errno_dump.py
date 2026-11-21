"""P1's errno dump: `os.strerror` text for `pycompat::pyerr` (design §12, plan GC-16 I-10).

    python3 -I port/difftest/oracle/errno_dump.py > linux.json   # pinned 3.12 image (glibc)
    python3 -I port/difftest/oracle/errno_dump.py > macos.json   # on macOS (libc's text)
    anthology-dev gen-errno < linux.json > crates/anthology-pycompat/src/errno_tables/linux_glibc.rs
    anthology-dev gen-errno < macos.json > crates/anthology-pycompat/src/errno_tables/macos.rs

`os.strerror` is the C library's `strerror`, so any Python 3.9+ gives the same text on one OS.
One JSON object on stdout:

- `"os"`: `"linux"` or `"macos"`;
- `"text"`: `[os.strerror(n) for n in range(0, max + 1)]`, where `max` is the last errno the C
  library knows (the last whose text is not the unknown form);
- `"unknown"`: the unknown form's prefix (glibc `"Unknown error "`, macOS `"Unknown error: "`),
  found from errno 10000 and checked on every probe in `"probes"`: `[[n, os.strerror(n)], ...]`
  for numbers past `max`, and -1.

Python 3.9+ and the standard library only. The output names nothing about the machine.
"""
from __future__ import annotations

import json
import os
import sys

PROBE = 10000


def main():
    if sys.platform.startswith("linux"):
        name = "linux"
    elif sys.platform == "darwin":
        name = "macos"
    else:
        raise SystemExit("errno_dump: only Linux and macOS have errno tables")
    sample = os.strerror(PROBE)
    if not sample.endswith(str(PROBE)):
        raise SystemExit("errno_dump: the unknown-errno text does not end in the number")
    prefix = sample[: -len(str(PROBE))]
    last = max(n for n in range(1, 1000) if os.strerror(n) != prefix + str(n))
    text = [os.strerror(n) for n in range(0, last + 1)]
    probes = [[n, os.strerror(n)] for n in (last + 1, last + 2, 255, 4096, 65535, 2**31 - 1, -1)]
    for n, t in probes:
        if t != prefix + str(n):
            raise SystemExit("errno_dump: errno %d is not in the unknown form: %r" % (n, t))
    json.dump({"os": name, "text": text, "unknown": prefix, "probes": probes}, sys.stdout,
              ensure_ascii=True, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
