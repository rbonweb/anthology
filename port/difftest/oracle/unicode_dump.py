"""P1's Unicode dump: the CPython `str` and `unicodedata` facts `pycompat` needs (design §12).

    python3 -I port/difftest/oracle/unicode_dump.py > dump.json      # in the pinned 3.12 image
    scripts/dev.sh gen-unicode < dump.json > crates/anthology-pycompat/src/tables.rs

One JSON object on stdout, computed from this interpreter alone, for every code point
0..=0x10FFFF except the surrogates (Rust `char`s cannot be surrogates):

- `"version"`: `unicodedata.unidata_version` (the pinned 3.12 image has 15.0.0).
- `"ranges"`: `{name: [[lo, hi], ...]}`, the sorted inclusive runs of code points for which the
  predicate holds: `isspace`, `isdecimal`, `isdigit`, `isalpha`, `isalnum`, `isprintable`,
  `word` (`isalnum() or c == "_"`: what a str pattern's `\\w` matches), and the two Final_Sigma
  properties CPython keeps private:
  - with P1 = `(c + "\\u03a3").lower()` ending in final sigma (c is cased and not
    case-ignorable) and P2 = `("A" + c + "\\u03a3").lower()` ending in final sigma (c is
    case-ignorable or cased): `case_ignorable` = P2 and not P1, `cased` = P1. CPython only asks
    `cased` of a character that is not case-ignorable (`handle_capital_sigma`), so that is all
    `lower` needs. A third probe, `("A\\u03a3" + c).lower()[1]`, is checked to agree.
- `"category"`: `[[lo, hi, cat], ...]` runs of `unicodedata.category`, without `Cn` runs.
- `"decimal"`: `[[lo, hi, value_at_lo], ...]` runs where `unicodedata.decimal` counts up by one.
- `"lower"`: `[[cp, str], ...]` for every code point whose `str.lower()` is not itself (one or
  more characters; U+03A3's entry is its non-final form).
- What `re.IGNORECASE` uses (`re._compiler`): `ranges["sre_cased"]` (`_sre.unicode_iscased`),
  `"sre_lower"`: `[[cp, lower], ...]` where `_sre.unicode_tolower` (the simple lowercase)
  differs from `"lower"` (only where `str.lower()` is more than one character), and
  `"extra_cases"`: `[[lower, [other lowers...]], ...]` (`re._casefix._EXTRA_CASES`). The dump
  checks that `\\w`, `\\s` and `\\d` give the same answer for a character and its simple
  lowercase, which `pycompat::re` relies on for classes under IGNORECASE.

Python 3.11+ (`re._casefix`) and the standard library only. The output names nothing about the
machine.
"""
from __future__ import annotations

import _sre
import json
import re
import sys
import unicodedata
from re import _casefix

MAX = 0x110000
SIGMA = "Σ"
FINAL = "ς"


def _points():
    for cp in range(MAX):
        if 0xD800 <= cp <= 0xDFFF:
            continue
        yield cp


def _runs(pred):
    out = []
    for cp in _points():
        if pred(chr(cp)):
            if out and out[-1][1] == cp - 1:
                out[-1][1] = cp
            else:
                out.append([cp, cp])
    return out


def _final(text, i):
    return text.lower()[i] == FINAL


def _case_ignorable(c):
    # P1 = cased and not ignorable; P2 = ignorable or cased (see the module docs).
    return not _final(c + SIGMA, -1) and _final("A" + c + SIGMA, -1)


def _cased(c):
    return _final(c + SIGMA, -1)


def _check(c):
    # For a character that is not case-ignorable, a following one ends a final sigma exactly
    # when it is cased: the third probe agrees with the first two.
    if not _case_ignorable(c) and _final("A" + SIGMA + c, 1) == _cased(c):
        raise SystemExit("unicode_dump: the Final_Sigma probes disagree at U+%04X" % ord(c))


def main():
    preds = {
        "isspace": str.isspace,
        "isdecimal": str.isdecimal,
        "isdigit": str.isdigit,
        "isalpha": str.isalpha,
        "isalnum": str.isalnum,
        "isprintable": str.isprintable,
        "word": lambda c: c.isalnum() or c == "_",
        "case_ignorable": _case_ignorable,
        "cased": _cased,
        "sre_cased": lambda c: _sre.unicode_iscased(ord(c)),
    }
    for cp in _points():
        _check(chr(cp))
    ranges = {name: _runs(pred) for name, pred in preds.items()}

    category = []
    for cp in _points():
        cat = unicodedata.category(chr(cp))
        if cat == "Cn":
            continue
        if category and category[-1][2] == cat and category[-1][1] == cp - 1:
            category[-1][1] = cp
        else:
            category.append([cp, cp, cat])

    decimal = []
    for cp in _points():
        v = unicodedata.decimal(chr(cp), None)
        if v is None:
            continue
        if decimal and decimal[-1][1] == cp - 1 and decimal[-1][2] + (cp - decimal[-1][0]) == v:
            decimal[-1][1] = cp
        else:
            decimal.append([cp, cp, v])

    lower = []
    for cp in _points():
        c = chr(cp)
        low = c.lower()
        if low != c:
            lower.append([cp, low])

    sre_lower = []
    classes = [re.compile(p) for p in (r"\w", r"\s", r"\d")]
    for cp in _points():
        c = chr(cp)
        low = _sre.unicode_tolower(cp)
        full = c.lower()
        if len(full) != 1 or ord(full) != low:
            if len(full) == 1:
                raise SystemExit("unicode_dump: U+%04X: the simple and full lowercase differ" % cp)
            sre_lower.append([cp, low])
        for r in classes:
            if bool(r.fullmatch(c)) != bool(r.fullmatch(chr(low))):
                raise SystemExit("unicode_dump: U+%04X: %s differs on its lowercase" % (cp, r.pattern))
    extra = [[k, list(v)] for k, v in sorted(_casefix._EXTRA_CASES.items())]

    json.dump(
        {
            "extra_cases": extra,
            "sre_lower": sre_lower,
            "version": unicodedata.unidata_version,
            "ranges": ranges,
            "category": category,
            "decimal": decimal,
            "lower": lower,
        },
        sys.stdout,
        ensure_ascii=True,
        sort_keys=True,
    )
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
