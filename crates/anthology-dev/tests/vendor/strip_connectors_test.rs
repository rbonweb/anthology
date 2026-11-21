//! test_vendor.StripConnectorsTest.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anthology_dev::vendor;
use anthology_pycompat::json::{self, Value};

use crate::fixture::{Fixture, MIT_TEXT, jv, write};

pub const CONNECTOR_DOC: &str = "\
---
name: demo
description: Demo.
---

# /demo

> If you see unfamiliar placeholders or need to check which tools are connected, see [CONNECTORS.md](../../CONNECTORS.md).

Intro paragraph.

## Steps

### 1. Gather

If **~~project tracker** is connected:
- Pull the tickets
- Link them

If no tracker is connected:
- Ask the user

**If ~~SEO tools are connected:**
- Pull keywords

**If ~~product analytics are connected:**
- Cross-reference traffic

Keep this paragraph.

- **~~knowledge base** (if connected): Search the wiki
- **Pasted text**: Notes

If ~~product analytics is connected, reference benchmarks.

Last sentence of the step.

```
## If Connectors Available
kept inside a fence
```

## If Connectors Available

If **~~knowledge base** is connected:
- Search prior docs

### Nested heading inside the section

Still connector text.

## Tool Integration

### If ~~email marketing is connected (e.g., Klaviyo)
- Set up the flow

### If no tools are connected
- Paste the copy

## Tips

1. Keep this tip.
";

const CONNECTOR_DOC_STRIPPED: &str = "\
---
name: demo
description: Demo.
---

# /demo

Intro paragraph.

## Steps

### 1. Gather

If no tracker is connected:
- Ask the user

Keep this paragraph.

- **Pasted text**: Notes

Last sentence of the step.

```
## If Connectors Available
kept inside a fence
```

## Tool Integration

### If no tools are connected
- Paste the copy

## Tips

1. Keep this tip.
";

const EMAIL_SEQUENCE: &str = concat!(
    "# Email Sequence\n\n## Sequence types\n\n**Educational Drip:**\nLesson 1 -- Lesson 2\n\n",
    "## Tool Integration\n\n",
    "### If ~~email marketing is connected (e.g., Klaviyo, Mailchimp)\n",
    "- Reference how to set up the flow\n- Map the branching logic\n\n",
    "### If ~~marketing automation or ~~CRM is connected (e.g., HubSpot)\n",
    "- Reference lead scoring data\n\n",
    "### If no tools are connected\n",
    "- Deliver all email content in copy-paste-ready format\n",
    "- Include a setup checklist the user can follow in any email platform:\n",
    "  1. Create the automation or flow\n  2. Set the enrollment trigger\n\n",
    "## Output\n\nPresent the complete sequence.\n"
);

const SUPERCHARGED: &str = concat!(
    "```\n+----+\n| STANDALONE (always works) |\n| SUPERCHARGED (when you connect your tools) |\n",
    "| + Tracker: link tickets |\n+----+\n```\n"
);

fn run_strip(f: &Fixture, files: &[(&str, &[u8])]) -> vendor::Result<(PathBuf, json::Map)> {
    let tree = f.tmp.join("tree");
    for (rel, text) in files {
        write(&tree, rel, text, false);
    }
    let stats = vendor::strip_connectors(&tree)?;
    Ok((tree, stats))
}

fn strip(doc: &str) -> String {
    let f = Fixture::new();
    let (tree, _) = run_strip(&f, &[("SKILL.md", doc.as_bytes())]).unwrap();
    fs::read_to_string(tree.join("SKILL.md")).unwrap()
}

fn stat(stats: &json::Map, k: &str) -> i64 {
    stats.get(k).and_then(Value::as_i64).unwrap()
}

#[test]
fn test_removes_connector_sections_blocks_and_references_only() {
    let f = Fixture::new();
    let (tree, stats) = run_strip(&f, &[("SKILL.md", CONNECTOR_DOC.as_bytes())]).unwrap();
    assert_eq!(
        fs::read_to_string(tree.join("SKILL.md")).unwrap(),
        CONNECTOR_DOC_STRIPPED
    );
    assert_eq!(stat(&stats, "files"), 1);
}

#[test]
fn test_headings_and_fences_are_respected() {
    // a heading-looking line inside a code fence neither starts nor ends a section
    let doc = "# T\n\n## If Connectors Available\n\ntext\n\n```\n## Not a heading\n```\n\nmore\n\n## Tips\n\nkept\n";
    assert_eq!(strip(doc), "# T\n\n## Tips\n\nkept\n");
}

#[test]
fn test_fenced_connector_text_is_not_stripped_but_still_fails_the_guard() {
    let f = Fixture::new();
    let r = run_strip(
        &f,
        &[("SKILL.md", b"# T\n\n```\nIf ~~x is connected:\n- y\n```\n")],
    );
    assert!(r.is_err());
}

#[test]
fn test_section_at_the_end_of_a_file_leaves_one_final_newline() {
    assert_eq!(
        strip("# T\n\ntext\n\n## Connectors\n\nIf ~~a is connected:\n- b\n"),
        "# T\n\ntext\n"
    );
}

#[test]
fn test_connect_setup_heading_with_a_url() {
    let doc = "# T\n\n## Connect UXKIN (optional)\n\nadd `https://uxkin.com/mcp`\n\n## 1. Start\n\nkept\n";
    assert_eq!(strip(doc), "# T\n\n## 1. Start\n\nkept\n");
}

#[test]
fn test_connected_tools_heading_is_a_connector_section() {
    let doc = "# T\n\n### 3. Pull Context from Connected Tools\n\nIf **~~a** is connected:\n- b\n\n### 4. Next\n\nkept\n";
    assert_eq!(strip(doc), "# T\n\n### 4. Next\n\nkept\n");
}

#[test]
fn test_tools_connected_blocks_without_a_placeholder() {
    // a one-line list item, a paragraph, a lead-in with a list after a blank line, a "weren't"
    let doc = concat!(
        "# T\n\n## A\n\n- Offer drafts\n- If project management tool is connected, offer updates\n\n",
        "## B\n\nIf the UXKIN tools are connected and the direction is open, call\n",
        "`find_ui_materials` for the feeling to see\nreal sites.\n\nNext paragraph.\n\n",
        "## C\n\nIf the UXKIN tools are connected:\n\n1. Call `find_ui_materials`\n   with words.\n",
        "2. Try again.\n\nWithout the tools, rely on the project.\n\n",
        "## D\n\nIf the UXKIN tools weren't connected and it would help, say once\n",
        "(setup: https://uxkin.com/ui-design-mcp).\n\nEnd.\n"
    );
    assert_eq!(
        strip(doc),
        concat!(
            "# T\n\n## A\n\n- Offer drafts\n\n## B\n\nNext paragraph.\n\n",
            "## C\n\nWithout the tools, rely on the project.\n\n## D\n\nEnd.\n"
        )
    );
}

#[test]
fn test_fallback_subsections_survive_inside_a_removed_connector_section() {
    // the email-sequence shape: the container and the connector children go; the
    // "If no tools are connected" subsection stays verbatim under the container heading
    assert_eq!(
        strip(EMAIL_SEQUENCE),
        concat!(
            "# Email Sequence\n\n## Sequence types\n\n**Educational Drip:**\nLesson 1 -- Lesson 2\n\n",
            "## Tool Integration\n\n",
            "### If no tools are connected\n",
            "- Deliver all email content in copy-paste-ready format\n",
            "- Include a setup checklist the user can follow in any email platform:\n",
            "  1. Create the automation or flow\n  2. Set the enrollment trigger\n\n",
            "## Output\n\nPresent the complete sequence.\n"
        )
    );
}

#[test]
fn test_fallback_paragraphs_survive_inside_a_removed_connector_section() {
    // the write-spec shape
    let doc = concat!(
        "# T\n\n### 2. Gather\n\nAsk.\n\n### 3. Pull Context from Connected Tools\n\n",
        "If **~~project tracker** is connected:\n- Search tickets\n\n",
        "If **~~design** is connected:\n- Pull mockups\n\n",
        "If these tools are not connected, work entirely from what the user provides. ",
        "Do not ask the user to connect tools.\n\n### 4. Generate\n\nkept\n"
    );
    assert_eq!(
        strip(doc),
        concat!(
            "# T\n\n### 2. Gather\n\nAsk.\n\n### 3. Pull Context from Connected Tools\n\n",
            "If these tools are not connected, work entirely from what the user provides. ",
            "Do not ask the user to connect tools.\n\n### 4. Generate\n\nkept\n"
        )
    );
}

#[test]
fn test_a_removed_section_without_fallbacks_leaves_nothing_and_a_fallback_at_the_end_stays() {
    assert_eq!(
        strip("# T\n\n## Connectors\n\nIf ~~a is connected:\n- b\n\n## Next\n\nx\n"),
        "# T\n\n## Next\n\nx\n"
    );
    assert_eq!(
        strip(
            "# T\n\n## Connectors\n\nIf ~~a is connected:\n- b\n\n### If no tools are connected\n- c\n"
        ),
        "# T\n\n## Connectors\n\n### If no tools are connected\n- c\n"
    );
}

#[test]
fn test_a_fallback_heading_is_not_a_connector_section_by_itself() {
    let doc = "# T\n\n### If no connected tools\n- Ask the user\n\n### Next\n\nx\n";
    assert_eq!(strip(doc), doc);
}

#[test]
fn test_a_longer_fence_is_not_closed_by_a_shorter_one() {
    let doc = "# T\n\n## Connectors\n\n````\n```\n## Not a heading\n```\n````\n\n## Tips\n\nkept\n";
    assert_eq!(strip(doc), "# T\n\n## Tips\n\nkept\n");
}

#[test]
fn test_no_tool_fallbacks_stay() {
    let doc = concat!(
        "# T\n\nIf no project management tool is connected:\n- Ask the user\n\n",
        "**If tools are not connected:**\n- Use web search\n\n",
        "If these tools are not connected, work from what the user provides.\n"
    );
    assert_eq!(strip(doc), doc);
}

#[test]
fn test_a_list_item_telling_the_user_to_connect_via_mcp_goes() {
    let doc = concat!(
        "# T\n\n**If tools are not connected:**\n- Use web search\n",
        "- Note: \"For precise data, connect an SEO tool like Ahrefs via MCP.\"\n\nNext.\n"
    );
    assert_eq!(
        strip(doc),
        "# T\n\n**If tools are not connected:**\n- Use web search\n\nNext.\n"
    );
}

#[test]
fn test_supercharged_box_goes_with_a_heading_it_leaves_empty() {
    let doc = format!("# T\n\nIntro.\n\n## How It Works\n\n{SUPERCHARGED}\n## Next\n\nkept\n");
    assert_eq!(strip(&doc), "# T\n\nIntro.\n\n## Next\n\nkept\n");
}

#[test]
fn test_supercharged_box_keeps_a_heading_that_has_other_content() {
    let doc = format!("# T\n\n## How It Works\n\nSome words.\n\n{SUPERCHARGED}\nAfter.\n");
    assert_eq!(
        strip(&doc),
        "# T\n\n## How It Works\n\nSome words.\n\nAfter.\n"
    );
    let doc = format!("# T\n\n## How It Works\n\n{SUPERCHARGED}\n### Sub\n\nkept\n");
    assert_eq!(strip(&doc), "# T\n\n## How It Works\n\n### Sub\n\nkept\n");
}

#[test]
fn test_supercharged_box_takes_a_line_that_only_introduces_it() {
    let doc = format!("# T\n\nText.\n\nHere is how it works:\n\n{SUPERCHARGED}\nAfter.\n");
    assert_eq!(strip(&doc), "# T\n\nText.\n\nAfter.\n");
}

#[test]
fn test_other_fenced_blocks_are_untouched() {
    let doc = "# T\n\n```\n+--+\n| STANDALONE |\n+--+\n```\n\nIf the tool is fine.\n";
    assert_eq!(strip(doc), doc);
}

fn md_files(tree: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![tree.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "md") {
                out.insert(p.display().to_string(), fs::read(&p).unwrap());
            }
        }
    }
    out
}

#[test]
fn test_idempotent() {
    let extra = format!(
        "{CONNECTOR_DOC}\n## Box\n\n{SUPERCHARGED}\n## Z\n\nIf the UXKIN tools are connected:\n\n1. a\n\nEnd.\n"
    );
    let f = Fixture::new();
    let (tree, _) = run_strip(
        &f,
        &[
            ("SKILL.md", extra.as_bytes()),
            ("references/a.md", CONNECTOR_DOC.as_bytes()),
        ],
    )
    .unwrap();
    let once = md_files(&tree);
    let stats = vendor::strip_connectors(&tree).unwrap();
    assert_eq!(once, md_files(&tree));
    assert_eq!(stat(&stats, "files"), 0);
}

#[test]
fn test_files_without_connectors_are_byte_identical() {
    let raw: &[u8] = b"# Plain\r\n\r\nNo connectors here.\r\n";
    let f = Fixture::new();
    let (tree, stats) = run_strip(
        &f,
        &[
            ("SKILL.md", raw),
            ("scripts/run.sh", b"#!/bin/sh\n~~ not markdown\n"),
        ],
    )
    .unwrap();
    assert_eq!(fs::read(tree.join("SKILL.md")).unwrap(), raw);
    assert_eq!(
        fs::read_to_string(tree.join("scripts/run.sh")).unwrap(),
        "#!/bin/sh\n~~ not markdown\n"
    );
    assert_eq!(stat(&stats, "files"), 0);
}

#[test]
fn test_deletes_a_connectors_file_in_the_tree() {
    let f = Fixture::new();
    let (tree, stats) = run_strip(
        &f,
        &[
            ("SKILL.md", b"# T\n"),
            ("CONNECTORS.md", b"# Connectors\n"),
            ("references/CONNECTORS.md", b"x\n"),
        ],
    )
    .unwrap();
    assert!(!tree.join("CONNECTORS.md").exists());
    assert!(!tree.join("references/CONNECTORS.md").exists());
    assert_eq!(stat(&stats, "deleted"), 2);
}

#[test]
fn test_unhandled_placeholder_fails_loudly_instead_of_guessing() {
    for doc in [
        "Open the ~~project tracker and look.\n",
        "See [x](CONNECTORS.md) for details.\n",
    ] {
        let f = Fixture::new();
        let err = run_strip(&f, &[("SKILL.md", doc.as_bytes())]).unwrap_err();
        assert!(err.0.contains("SKILL.md:1"), "{}", err.0);
    }
}

#[test]
fn test_preserves_the_executable_bit() {
    let f = Fixture::new();
    let tree = f.tmp.join("tree");
    write(
        &tree,
        "x.md",
        "# T\n\n## Connectors\n\nIf ~~a is connected:\n- b\n",
        true,
    );
    vendor::strip_connectors(&tree).unwrap();
    let mode = fs::metadata(tree.join("x.md"))
        .unwrap()
        .permissions()
        .mode();
    assert!(mode & 0o111 != 0);
}

#[test]
fn test_pipeline_applies_it_from_sources_json() {
    let f = Fixture::new();
    let e = f.add_upstream(
        "conn",
        &[("LICENSE", MIT_TEXT), ("SKILL.md", CONNECTOR_DOC)],
        &[("transform", jv(r#"["strip-connectors"]"#))],
    );
    f.build_all(&[e]);
    assert_eq!(f.check(), Vec::<String>::new());
    let shipped = fs::read_to_string(f.target("conn", "SKILL.md")).unwrap();
    assert!(!shipped.contains("~~"));
    assert!(!shipped.contains("CONNECTORS.md"));
}

// ---- parity details beyond the Python tests

#[test]
fn python_whitespace_and_error_text() {
    // \x1c is whitespace for str.strip(); the error quotes line.strip()[:100].
    assert_eq!(
        vendor::strip_markdown("# T\n\u{1c}\n## Connectors\n\nx\n").unwrap(),
        "# T\n"
    );
    let f = Fixture::new();
    let long = format!("  ~~{}  \n", "é".repeat(150));
    let err = run_strip(&f, &[("a.MD", long.as_bytes())]).unwrap_err();
    assert_eq!(
        err.0,
        format!(
            "strip-connectors: a.MD:1: connector markup the transform does not handle (failing on \
             purpose, so nothing slips through; extend strip-connectors): ~~{}",
            "é".repeat(98)
        )
    );
}

#[test]
fn ignorecase_folds_dotted_and_dotless_i_like_python() {
    // Python's IGNORECASE matches U+0130 and U+0131 to `i` (controller ruling (a)).
    for doc in [
        "# T\n\n## Tool Integratıon\n\nx\n\n## Next\n\ny\n",
        "# T\n\n## TOOL İNTEGRATION\n\nx\n\n## Next\n\ny\n",
    ] {
        assert_eq!(
            vendor::strip_markdown(doc).unwrap(),
            "# T\n\n## Next\n\ny\n"
        );
    }
    let doc = "# T\n\n## Connectors\n\nx\n\n### İf no tools are connected\n- c\n";
    assert_eq!(
        vendor::strip_markdown(doc).unwrap(),
        "# T\n\n## Connectors\n\n### İf no tools are connected\n- c\n"
    );
}
