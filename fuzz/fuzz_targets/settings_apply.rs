//! Fuzz target `settings_apply` (ST1, design §13.5): the settings patcher never panics on any
//! text, and for every text `plan` accepts with the setup op set, applying the plan and
//! reverting its records gives back the original bytes. `redact` and a `reconcile` over the
//! stored records run too, for panics.
#![no_main]

use anthology_pycompat::json::Value;
use anthology_settings::{
    ALLOW_ATL, ALLOW_MEM_SAVE, Op, TAG_MEM_SAVE, TAG_PLUGIN_READ, TAG_TOKEN_LAYER, apply, plan,
    reconcile, redact, revert,
};

/// The shape of `setup_ops` without a plugin folder on disk.
fn setup_like_ops() -> Vec<Op> {
    vec![
        Op::add(&["permissions", "allow"], ALLOW_ATL).with_tag(TAG_TOKEN_LAYER),
        Op::add(&["permissions", "allow"], ALLOW_MEM_SAVE).with_tag(TAG_MEM_SAVE),
        Op::add(&["permissions", "deny"], "Read(~/.config/chromium)"),
        Op::add(&["permissions", "deny"], "Read(~/.config/chromium/**)"),
        Op::add(&["permissions", "allow"], "Read(//opt/plugins/**)").with_tag(TAG_PLUGIN_READ),
        Op::Set {
            path: vec!["worktree".into(), "baseRef".into()],
            value: Value::Str("head".into()),
            tag: None,
        },
    ]
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = redact(text);
    let ops = setup_like_ops();
    if let Ok(planned) = plan(text, &ops) {
        let (new, _) = apply(text, &planned.ops).expect("apply succeeds after plan");
        assert_eq!(
            revert(&new, &planned.stored).expect("revert succeeds"),
            text,
            "apply then revert must restore the bytes"
        );
        let moved: Vec<Op> = ops
            .iter()
            .map(|o| match o.tag() {
                Some(TAG_PLUGIN_READ) => {
                    o.clone().with_value(Value::Str("Read(//srv/p/**)".into()))
                }
                _ => o.clone(),
            })
            .collect();
        let _ = reconcile(&new, &moved, &planned.stored);
    }
});
