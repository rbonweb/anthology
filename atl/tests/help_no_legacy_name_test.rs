//! ATL's own help must never print the name of the project it was forked from.
//!
//! The walk starts at `atl --help`, reads the `Commands:` list clap prints, and runs
//! `atl <sub> --help` for each entry, then again for the entries those print. Tools that
//! ATL wraps print their own pages: those never carry the legacy name either, so the walk
//! does not need to tell the two apart.

mod common;

use std::process::Stdio;

/// The name ATL replaced. The one place this test names it.
const LEGACY_NAME: &str = "rtk"; // legacy
/// What the old name stood for.
const LEGACY_TITLE: &str = "token killer"; // legacy

/// Help text for `atl <path...> --help`, stdout and stderr together.
fn help(path: &[String]) -> String {
    let out = common::atl_command()
        .args(path)
        .arg("--help")
        .stdin(Stdio::null())
        .output()
        .expect("atl should start");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The subcommand names in a clap help page: the first word of each indented line under
/// `Commands:`, up to the next blank line. `help` is clap's own and is skipped.
fn subcommands(page: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut inside = false;
    for line in page.lines() {
        if line.trim_end() == "Commands:" {
            inside = true;
        } else if inside && line.trim().is_empty() {
            break;
        } else if inside
            && line.starts_with("  ")
            && let Some(name) = line.split_whitespace().next()
            && name != "help"
        {
            names.push(name.to_string());
        }
    }
    names
}

fn walk(path: &mut Vec<String>, depth: usize, pages: &mut Vec<(String, String)>) {
    let page = help(path);
    let children = subcommands(&page);
    pages.push((path.join(" "), page));
    if depth == 0 {
        return;
    }
    for child in children {
        path.push(child);
        walk(path, depth - 1, pages);
        path.pop();
    }
}

#[test]
fn help_output_has_no_legacy_name() {
    let mut pages = Vec::new();
    walk(&mut Vec::new(), 2, &mut pages);
    assert!(
        pages.len() > 10,
        "the walk found only {} help pages: the Commands list was not read",
        pages.len()
    );
    let leaks: Vec<String> = pages
        .iter()
        .flat_map(|(path, page)| {
            page.lines()
                .filter(|l| {
                    let lower = l.to_lowercase();
                    lower.contains(LEGACY_NAME) || lower.contains(LEGACY_TITLE)
                })
                .map(move |l| format!("atl {path} --help: {l}"))
        })
        .collect();
    assert!(
        leaks.is_empty(),
        "help names the old tool:\n{}",
        leaks.join("\n")
    );
}
