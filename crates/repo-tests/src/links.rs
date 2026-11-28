//! Relative-link checker for knowledge folders: finds links in Markdown files that point at a file
//! or folder that does not exist, so an agent is never sent to a missing reference.

use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

/// `${CLAUDE_SKILL_DIR}/<rel>`: a path in the skill's own folder, wherever the file that names it is.
static SKILL_DIR_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{CLAUDE_SKILL_DIR\}/([A-Za-z0-9_./-]+)").expect("valid regex")
});

/// `](<target>`: the target of a Markdown link or image, up to a space (a title) or the `)`.
static MARKDOWN_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\]\(([^)\s]+)").expect("valid regex"));

/// `dangling_links_in` over the repository root.
pub fn dangling_links(folder_rel: &str) -> Vec<String> {
    dangling_links_in(&crate::repo::root(), folder_rel)
}

/// The dangling links in every `*.md` below `root/folder_rel`, as `<file>:<line>: <target>`
/// (the file relative to `root`, the target as written), sorted. Panics when the folder cannot be
/// read, so a misspelt folder never passes as "no dangling links".
///
/// - Lines inside fenced code blocks are not checked.
/// - A `${CLAUDE_SKILL_DIR}/<rel>` path (a trailing `.` is the end of a sentence, not the path)
///   resolves against `folder_rel`.
/// - A Markdown link target resolves against the folder of the file that holds it, after its
///   `#fragment` is cut. It is skipped when it is a `http://`, `https://` or `mailto:` address,
///   starts with `#`, or holds `<` or `{` (a placeholder). One starting with `/` always dangles:
///   a knowledge file has no root to resolve it against.
/// - A target is found when it is a file or a folder.
pub fn dangling_links_in(root: &Path, folder_rel: &str) -> Vec<String> {
    let mut files = Vec::new();
    markdown_files(root, folder_rel, &mut files);
    let skill_dir = root.join(folder_rel);
    let mut found = Vec::new();
    for file in files {
        let path = root.join(&file);
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let own_dir = path.parent().unwrap_or(root);
        let mut fence = None;
        for (index, line) in text.lines().enumerate() {
            if in_fence(&mut fence, line) {
                continue;
            }
            let mut report = |target: &str| found.push(format!("{file}:{}: {target}", index + 1));
            for caps in SKILL_DIR_LINK.captures_iter(line) {
                let rel = caps[1].trim_end_matches('.');
                if !skill_dir.join(rel).exists() {
                    report(&format!("${{CLAUDE_SKILL_DIR}}/{rel}"));
                }
            }
            for caps in MARKDOWN_LINK.captures_iter(line) {
                let target = &caps[1];
                if is_not_a_file_link(target) {
                    continue;
                }
                let without_fragment = target.split('#').next().unwrap_or_default();
                if target.starts_with('/') || !own_dir.join(without_fragment).exists() {
                    report(target);
                }
            }
        }
    }
    found.sort();
    found
}

/// Pushes the repo-relative paths (`/`-separated) of the `*.md` files below `root/rel` onto `out`.
/// Links are not followed.
fn markdown_files(root: &Path, rel: &str, out: &mut Vec<String>) {
    let dir = root.join(rel);
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        let child = format!("{rel}/{}", entry.file_name().to_string_lossy());
        let kind = entry.file_type().unwrap_or_else(|e| panic!("{child}: {e}"));
        if kind.is_dir() {
            markdown_files(root, &child, out);
        } else if kind.is_file() && child.ends_with(".md") {
            out.push(child);
        }
    }
}

/// A link target that is not a path in the repository: an address, an anchor or a placeholder.
fn is_not_a_file_link(target: &str) -> bool {
    ["http://", "https://", "mailto:", "#"]
        .iter()
        .any(|prefix| target.starts_with(prefix))
        || target.contains(['<', '{'])
}

/// Tracks fenced code blocks (CommonMark: three or more backticks or tildes, closed by at least as
/// many of the same character; a block left open runs to the end of the file). `fence` is the open
/// block's character and length. True for a fence line and for every line inside a block.
fn in_fence(fence: &mut Option<(char, usize)>, line: &str) -> bool {
    let text = line.trim_start();
    let Some(mark) = text.chars().next().filter(|c| matches!(c, '`' | '~')) else {
        return fence.is_some();
    };
    let run = text.chars().take_while(|&c| c == mark).count();
    let rest = &text[run..];
    match *fence {
        Some((open, len)) => {
            if mark == open && run >= len && rest.trim().is_empty() {
                *fence = None;
            }
            true
        }
        // A backtick run followed by more backticks is a code span, not a fence.
        None if run >= 3 && !(mark == '`' && rest.contains('`')) => {
            *fence = Some((mark, run));
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// A scratch repository root holding `files` (`(path, text)`; an empty text is a file too).
    fn tree(files: &[(&str, &str)]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in files {
            let p = dir.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        dir
    }

    #[test]
    fn resolves_a_reference_link() {
        let t = tree(&[
            (
                "k/a/SKILL.md",
                "# A\n\nSee [the guide](references/guide.md).\n",
            ),
            ("k/a/references/guide.md", "guide\n"),
        ]);
        assert!(dangling_links_in(t.path(), "k/a").is_empty());
    }

    #[test]
    fn reports_a_missing_target() {
        let t = tree(&[(
            "k/a/SKILL.md",
            "# A\n\nSee [the guide](references/guide.md).\n",
        )]);
        assert_eq!(
            dangling_links_in(t.path(), "k/a"),
            ["k/a/SKILL.md:3: references/guide.md"]
        );
    }

    #[test]
    fn ignores_fenced_code() {
        let t = tree(&[(
            "k/a/SKILL.md",
            "```md\n[x](gone.md)\n```\n~~~\n[y](gone.md)\n~~~\n[z](after.md)\n",
        )]);
        // Only the link after both fences closed counts.
        assert_eq!(
            dangling_links_in(t.path(), "k/a"),
            ["k/a/SKILL.md:7: after.md"]
        );
    }

    #[test]
    fn a_shorter_fence_does_not_close_a_longer_one() {
        let t = tree(&[(
            "k/a/SKILL.md",
            "````\n```\n[x](gone.md)\n````\n[y](after.md)\n",
        )]);
        assert_eq!(
            dangling_links_in(t.path(), "k/a"),
            ["k/a/SKILL.md:5: after.md"]
        );
    }

    #[test]
    fn a_code_span_of_three_backticks_does_not_open_a_fence() {
        let t = tree(&[(
            "k/a/SKILL.md",
            "```inline``` then [x](gone.md)\n[y](gone2.md)\n",
        )]);
        assert_eq!(
            dangling_links_in(t.path(), "k/a"),
            ["k/a/SKILL.md:1: gone.md", "k/a/SKILL.md:2: gone2.md"]
        );
    }

    #[test]
    fn a_target_starting_with_a_slash_dangles_even_when_the_host_has_it() {
        let t = tree(&[("k/a/SKILL.md", "[x](/etc/hosts)\n")]);
        assert_eq!(
            dangling_links_in(t.path(), "k/a"),
            ["k/a/SKILL.md:1: /etc/hosts"]
        );
    }

    #[test]
    fn resolves_skill_dir_links_against_the_skill_folder() {
        let t = tree(&[
            (
                "k/blog-write/references/x.md",
                "Read `${CLAUDE_SKILL_DIR}/../blog/references/q.md` and \
                 `${CLAUDE_SKILL_DIR}/references/y.md`.\n",
            ),
            ("k/blog/references/q.md", "q\n"),
            ("k/blog-write/references/y.md", "y\n"),
        ]);
        assert!(dangling_links_in(t.path(), "k/blog-write").is_empty());
        fs::remove_file(t.path().join("k/blog/references/q.md")).unwrap();
        assert_eq!(
            dangling_links_in(t.path(), "k/blog-write"),
            ["k/blog-write/references/x.md:1: ${CLAUDE_SKILL_DIR}/../blog/references/q.md"]
        );
    }

    #[test]
    fn trims_the_dot_that_ends_a_sentence_after_a_skill_dir_path() {
        let t = tree(&[
            (
                "k/a/SKILL.md",
                "Read ${CLAUDE_SKILL_DIR}/references/y.md.\n",
            ),
            ("k/a/references/y.md", "y\n"),
        ]);
        assert!(dangling_links_in(t.path(), "k/a").is_empty());
    }

    #[test]
    fn skips_urls_and_placeholders() {
        let t = tree(&[(
            "k/a/SKILL.md",
            "[a](http://x.test/a) [b](https://x.test/b) [c](mailto:me@x.test) [d](#top) \
             [e](<path with spaces.md>) [f](${DIR}/x.md) [g](references/{name}.md)\n",
        )]);
        assert!(dangling_links_in(t.path(), "k/a").is_empty());
    }

    #[test]
    fn strips_fragments() {
        let t = tree(&[
            (
                "k/a/SKILL.md",
                "[ok](references/x.md#section) [bad](references/nope.md#section)\n",
            ),
            ("k/a/references/x.md", "x\n"),
        ]);
        assert_eq!(
            dangling_links_in(t.path(), "k/a"),
            ["k/a/SKILL.md:1: references/nope.md#section"]
        );
    }

    #[test]
    fn a_markdown_link_resolves_against_its_own_file_and_may_name_a_folder() {
        let t = tree(&[
            ("k/a/references/x.md", "[up](../SKILL.md) [dir](./sub/)\n"),
            ("k/a/references/sub/y.md", "y\n"),
            ("k/a/SKILL.md", "# A\n"),
        ]);
        assert!(dangling_links_in(t.path(), "k/a").is_empty());
    }

    #[test]
    fn lists_every_miss_sorted_and_checks_only_markdown_files() {
        let t = tree(&[
            ("k/a/z.md", "[x](gone.md)\n"),
            ("k/a/sub/b.md", "[x](gone.md)\n\n[y](gone2.md)\n"),
            ("k/a/notes.txt", "[x](gone.md)\n"),
            ("k/other/c.md", "[x](gone.md)\n"),
        ]);
        assert_eq!(
            dangling_links_in(t.path(), "k/a"),
            [
                "k/a/sub/b.md:1: gone.md",
                "k/a/sub/b.md:3: gone2.md",
                "k/a/z.md:1: gone.md",
            ]
        );
    }

    #[test]
    #[should_panic(expected = "k/nope")]
    fn a_missing_folder_is_an_error_not_an_empty_result() {
        let t = tree(&[("k/a/SKILL.md", "# A\n")]);
        dangling_links_in(t.path(), "k/nope");
    }
}
