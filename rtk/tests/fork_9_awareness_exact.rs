//! Fork change 9 (rbonweb/rtk 8a970b36): the awareness text `rtk init` installs
//! (`src/hooks/init/mod.rs` embeds these files with `include_str!`) tells agents
//! which outputs RTK keeps exact, so they stop adding `RTK_DISABLED=1` or
//! `rtk bypass` to get exact text.

const AWARENESS: [(&str, &str); 3] = [
    ("default", include_str!("../hooks/rtk-awareness.md")),
    ("high", include_str!("../hooks/rtk-awareness-high.md")),
    ("full", include_str!("../hooks/rtk-awareness-full.md")),
];

#[test]
fn fork_9_every_awareness_level_says_what_stays_exact() {
    for (level, text) in AWARENESS {
        for needle in [
            "Exact text stays exact:",
            "`grep -o` and `grep -c` results",
            "`jq -r` values",
            "`curl -o` / `-w` results come back unchanged",
            "in-place edits such as `sed -i` are never rewritten",
            "Do not add `RTK_DISABLED=1` or `rtk bypass` to get exact text.",
        ] {
            assert!(
                text.contains(needle),
                "{level} awareness text lacks `{needle}`:\n{text}"
            );
        }
    }
}
