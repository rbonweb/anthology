//! `pycompat::difflib`: CPython 3.12's `SequenceMatcher` and `unified_diff` (design §12).
//!
//! A port of `difflib.SequenceMatcher(None, a, b)` (no junk function, `autojunk=True`: when `b`
//! has 200 or more items, an item that appears more than `len(b) // 100 + 1` times is
//! "popular" and never starts a match) and of `difflib.unified_diff(..., lineterm="")` with no
//! file dates. The output equals `list(difflib.unified_diff(a, b, fromfile, tofile, n=n,
//! lineterm=""))`.

use std::collections::{HashMap, HashSet};

/// One `(tag, i1, i2, j1, j2)` opcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opcode {
    pub tag: Tag,
    pub i1: usize,
    pub i2: usize,
    pub j1: usize,
    pub j2: usize,
}

/// An opcode's tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Replace,
    Delete,
    Insert,
    Equal,
}

impl Tag {
    /// Python's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Tag::Replace => "replace",
            Tag::Delete => "delete",
            Tag::Insert => "insert",
            Tag::Equal => "equal",
        }
    }
}

/// `difflib.SequenceMatcher(None, a, b)`.
pub struct SequenceMatcher<'a> {
    a: &'a [&'a str],
    b: &'a [&'a str],
    /// Each non-popular item of `b`, with its indices in increasing order.
    b2j: HashMap<&'a str, Vec<usize>>,
}

impl<'a> SequenceMatcher<'a> {
    pub fn new(a: &'a [&'a str], b: &'a [&'a str]) -> SequenceMatcher<'a> {
        let mut b2j: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, elt) in b.iter().enumerate() {
            b2j.entry(*elt).or_default().push(i);
        }
        let n = b.len();
        if n >= 200 {
            let ntest = n / 100 + 1;
            let popular: HashSet<&str> = b2j
                .iter()
                .filter(|(_, idxs)| idxs.len() > ntest)
                .map(|(elt, _)| *elt)
                .collect();
            for elt in popular {
                b2j.remove(elt);
            }
        }
        SequenceMatcher { a, b, b2j }
    }

    /// `find_longest_match(alo, ahi, blo, bhi)`: `(i, j, k)`. There is no junk, so the
    /// junk-extension loops of CPython reduce to extending over equal (popular) items.
    pub fn find_longest_match(
        &self,
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> (usize, usize, usize) {
        let (a, b) = (self.a, self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, item) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = self.b2j.get(item) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        // CPython's isbjunk is always false here (no isjunk); popular items are not junk.
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    /// `get_matching_blocks()`, the `(len(a), len(b), 0)` sentinel included.
    pub fn get_matching_blocks(&self) -> Vec<(usize, usize, usize)> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0, la, 0, lb)];
        let mut blocks = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort_unstable();
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        let mut out = Vec::new();
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    out.push((i1, j1, k1));
                }
                (i1, j1, k1) = (i2, j2, k2);
            }
        }
        if k1 > 0 {
            out.push((i1, j1, k1));
        }
        out.push((la, lb, 0));
        out
    }

    /// `get_opcodes()`.
    pub fn get_opcodes(&self) -> Vec<Opcode> {
        let (mut i, mut j) = (0, 0);
        let mut out = Vec::new();
        for (ai, bj, size) in self.get_matching_blocks() {
            let tag = if i < ai && j < bj {
                Some(Tag::Replace)
            } else if i < ai {
                Some(Tag::Delete)
            } else if j < bj {
                Some(Tag::Insert)
            } else {
                None
            };
            if let Some(tag) = tag {
                out.push(Opcode {
                    tag,
                    i1: i,
                    i2: ai,
                    j1: j,
                    j2: bj,
                });
            }
            i = ai + size;
            j = bj + size;
            if size > 0 {
                out.push(Opcode {
                    tag: Tag::Equal,
                    i1: ai,
                    i2: i,
                    j1: bj,
                    j2: j,
                });
            }
        }
        out
    }

    /// `get_grouped_opcodes(n)`.
    pub fn get_grouped_opcodes(&self, n: usize) -> Vec<Vec<Opcode>> {
        let mut codes = self.get_opcodes();
        if codes.is_empty() {
            codes.push(Opcode {
                tag: Tag::Equal,
                i1: 0,
                i2: 1,
                j1: 0,
                j2: 1,
            });
        }
        if let Some(c) = codes.first_mut().filter(|c| c.tag == Tag::Equal) {
            c.i1 = c.i1.max(c.i2.saturating_sub(n));
            c.j1 = c.j1.max(c.j2.saturating_sub(n));
        }
        if let Some(c) = codes.last_mut().filter(|c| c.tag == Tag::Equal) {
            c.i2 = c.i2.min(c.i1 + n);
            c.j2 = c.j2.min(c.j1 + n);
        }
        let nn = n + n;
        let mut groups = Vec::new();
        let mut group = Vec::new();
        for mut c in codes {
            if c.tag == Tag::Equal && c.i2 - c.i1 > nn {
                group.push(Opcode {
                    tag: Tag::Equal,
                    i1: c.i1,
                    i2: c.i2.min(c.i1 + n),
                    j1: c.j1,
                    j2: c.j2.min(c.j1 + n),
                });
                groups.push(std::mem::take(&mut group));
                c.i1 = c.i1.max(c.i2.saturating_sub(n));
                c.j1 = c.j1.max(c.j2.saturating_sub(n));
            }
            group.push(c);
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].tag == Tag::Equal) {
            groups.push(group);
        }
        groups
    }
}

/// `difflib._format_range_unified(start, stop)`.
fn format_range_unified(start: usize, stop: usize) -> String {
    let mut beginning = start + 1;
    let length = stop - start;
    if length == 1 {
        return beginning.to_string();
    }
    if length == 0 {
        beginning -= 1;
    }
    format!("{beginning},{length}")
}

/// `list(difflib.unified_diff(a, b, fromfile, tofile, n=n, lineterm=""))`.
pub fn unified_diff(a: &[&str], b: &[&str], fromfile: &str, tofile: &str, n: usize) -> Vec<String> {
    unified_diff_lineterm(a, b, fromfile, tofile, n, "")
}

/// `list(difflib.unified_diff(a, b, fromfile, tofile, n=n, lineterm=lineterm))`: `lineterm`
/// ends the `---`, `+++` and `@@` lines (the content lines are `a`'s and `b`'s as given).
pub fn unified_diff_lineterm(
    a: &[&str],
    b: &[&str],
    fromfile: &str,
    tofile: &str,
    n: usize,
    lineterm: &str,
) -> Vec<String> {
    let m = SequenceMatcher::new(a, b);
    let mut out = Vec::new();
    for (k, group) in m.get_grouped_opcodes(n).iter().enumerate() {
        if k == 0 {
            out.push(format!("--- {fromfile}{lineterm}"));
            out.push(format!("+++ {tofile}{lineterm}"));
        }
        let (first, last) = (group[0], group[group.len() - 1]);
        out.push(format!(
            "@@ -{} +{} @@{lineterm}",
            format_range_unified(first.i1, last.i2),
            format_range_unified(first.j1, last.j2)
        ));
        for c in group {
            if c.tag == Tag::Equal {
                out.extend(a[c.i1..c.i2].iter().map(|l| format!(" {l}")));
                continue;
            }
            if matches!(c.tag, Tag::Replace | Tag::Delete) {
                out.extend(a[c.i1..c.i2].iter().map(|l| format!("-{l}")));
            }
            if matches!(c.tag, Tag::Replace | Tag::Insert) {
                out.extend(b[c.j1..c.j2].iter().map(|l| format!("+{l}")));
            }
        }
    }
    out
}
