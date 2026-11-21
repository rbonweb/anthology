//! `Op` and `setup_ops` (ST1): `settings_patch.py`'s op set, its constants and
//! `plugin_read_rule`.

use std::path::Path;

use anthology_core::Env;
use anthology_guard::data::{PREFIX_VARIANTS, PROFILE_ROOTS};
use anthology_pycompat::json::{Map, Value};

use crate::scanner::py;
use crate::validate::validate_plugin_root;

/// `BYPASS_NOTE` (GC-8).
pub const BYPASS_NOTE: &str = "Note: `Bash(rtk *)` also auto-approves `rtk bypass <cmd>`, which runs any command. rtk's hook still applies the deny and ask rules in your settings files (user, project, local and managed) to the command it wraps, but not rules given with `--settings` or `--disallowedTools`, server-managed settings or an MDM profile.";
/// `ALLOW_RTK`.
pub const ALLOW_RTK: &str = "Bash(rtk *)";
/// `ALLOW_MEM_SAVE`: lets read-only agents save memory notes.
pub const ALLOW_MEM_SAVE: &str = "Bash(anthology-mem save *)";
/// `TAG_PLUGIN_READ`.
pub const TAG_PLUGIN_READ: &str = "plugin-read";
/// `TAG_MEM_SAVE`.
pub const TAG_MEM_SAVE: &str = "mem-save";
/// `_REPLACEABLE_TAGS`: ops whose value may change between runs; a re-run replaces the value
/// it added earlier for the same tag.
pub const REPLACEABLE_TAGS: [&str; 2] = [TAG_PLUGIN_READ, TAG_MEM_SAVE];
/// `OPS_VERSION`, the default of [`SettingsLimits::ops_version`].
pub const OPS_VERSION: i64 = 1;
/// `REDACTED`.
pub const REDACTED: &str = "<redacted>";
/// `SHOWN_PATHS`: the only places whose values a diff shows.
pub const SHOWN_PATHS: [&[&str]; 3] = [
    &["permissions", "allow"],
    &["permissions", "deny"],
    &["worktree", "baseRef"],
];

/// A settings op (GC-8): `{"op": "add" | "set", "path": [...], "value": v[, "tag": t]}`.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Append `value` to the list at `path` (a no-op when present; creates containers).
    Add {
        path: Vec<String>,
        value: Value,
        tag: Option<String>,
    },
    /// Set the key at `path` to `value` (remembers the previous raw value).
    Set {
        path: Vec<String>,
        value: Value,
        tag: Option<String>,
    },
}

impl Op {
    /// An untagged `add`.
    pub fn add(path: &[&str], value: impl Into<String>) -> Op {
        Op::Add {
            path: path.iter().map(|s| s.to_string()).collect(),
            value: Value::Str(value.into()),
            tag: None,
        }
    }

    /// `"add"` or `"set"`.
    pub fn kind(&self) -> &'static str {
        match self {
            Op::Add { .. } => "add",
            Op::Set { .. } => "set",
        }
    }

    pub fn path(&self) -> &[String] {
        match self {
            Op::Add { path, .. } | Op::Set { path, .. } => path,
        }
    }

    pub fn value(&self) -> &Value {
        match self {
            Op::Add { value, .. } | Op::Set { value, .. } => value,
        }
    }

    pub fn tag(&self) -> Option<&str> {
        match self {
            Op::Add { tag, .. } | Op::Set { tag, .. } => tag.as_deref(),
        }
    }

    /// The same op with `tag`.
    pub fn with_tag(mut self, t: &str) -> Op {
        match &mut self {
            Op::Add { tag, .. } | Op::Set { tag, .. } => *tag = Some(t.to_string()),
        }
        self
    }

    /// The same op with `value`.
    pub fn with_value(mut self, v: Value) -> Op {
        match &mut self {
            Op::Add { value, .. } | Op::Set { value, .. } => *value = v,
        }
        self
    }

    /// The op as Python's dict: `op`, `path`, `value`, then `tag` when there is one.
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("op".into(), Value::Str(self.kind().into()));
        m.insert(
            "path".into(),
            Value::Array(self.path().iter().map(|p| Value::Str(p.clone())).collect()),
        );
        m.insert("value".into(), self.value().clone());
        if let Some(t) = self.tag() {
            m.insert("tag".into(), Value::Str(t.into()));
        }
        Value::Object(m)
    }

    /// An op from its dict (`op` `add`/`set`, a list-of-str `path`, a `value`, an optional
    /// str `tag`; other keys are ignored). `None` for anything else.
    pub fn from_value(v: &Value) -> Option<Op> {
        let m = v.as_object()?;
        let path = m
            .get("path")?
            .as_array()?
            .iter()
            .map(|p| p.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()?;
        let value = m.get("value")?.clone();
        let tag = match m.get("tag") {
            None => None,
            Some(t) => Some(t.as_str()?.to_string()),
        };
        match m.get("op")?.as_str()? {
            "add" => Some(Op::Add { path, value, tag }),
            "set" => Some(Op::Set { path, value, tag }),
            _ => None,
        }
    }
}

/// GC-19 limits: the constants tests patch.
#[derive(Clone, Debug)]
pub struct SettingsLimits {
    /// `OPS_VERSION`.
    pub ops_version: i64,
}

impl Default for SettingsLimits {
    fn default() -> SettingsLimits {
        SettingsLimits {
            ops_version: OPS_VERSION,
        }
    }
}

/// GC-19 seams for the callers (session start and setup patch `settings_patch.setup_ops`).
#[derive(Clone, Copy, Debug)]
pub struct SettingsDeps {
    pub setup_ops: fn(&Path, &Env) -> Result<Vec<Op>, String>,
}

impl Default for SettingsDeps {
    fn default() -> SettingsDeps {
        SettingsDeps { setup_ops }
    }
}

/// `plugin_read_rule(plugin_root)`: `Read(//<parent of the real plugin root>/**)`.
pub fn plugin_read_rule(plugin_root: &Path) -> String {
    let real = py::realpath_os(plugin_root.as_os_str());
    let parent = py::dirname(&real.to_string_lossy());
    format!("Read(//{}/**)", parent.trim_start_matches('/'))
}

/// `setup_ops(plugin_root, env=env)`: everything `/anthology:setup` changes in settings.json.
/// The profile roots and their `.config/` channel variants come from the guard's data (I-2).
pub fn setup_ops(plugin_root: &Path, env: &Env) -> Result<Vec<Op>, String> {
    setup_ops_with(plugin_root, env, None)
}

/// `setup_ops(plugin_root, profile_roots, env)`: injected `profile_roots` get no channel
/// variants, as in Python.
pub fn setup_ops_with(
    plugin_root: &Path,
    env: &Env,
    profile_roots: Option<&[&str]>,
) -> Result<Vec<Op>, String> {
    validate_plugin_root(plugin_root, env)?;
    let (roots, variants): (&[&str], &[&str]) = match profile_roots {
        Some(r) => (r, &[""]),
        None => (&PROFILE_ROOTS, &PREFIX_VARIANTS),
    };
    let mut ops = vec![
        Op::add(&["permissions", "allow"], ALLOW_RTK),
        Op::add(&["permissions", "allow"], ALLOW_MEM_SAVE).with_tag(TAG_MEM_SAVE),
    ];
    for root in roots {
        let vs: &[&str] = if root.starts_with(".config/") {
            variants
        } else {
            &[""]
        };
        for v in vs {
            for rule in [
                format!("Read(~/{root}{v})"),
                format!("Read(~/{root}{v}/**)"),
            ] {
                ops.push(Op::add(&["permissions", "deny"], rule));
            }
        }
    }
    ops.push(
        Op::add(&["permissions", "allow"], plugin_read_rule(plugin_root)).with_tag(TAG_PLUGIN_READ),
    );
    ops.push(Op::Set {
        path: vec!["worktree".into(), "baseRef".into()],
        value: Value::Str("head".into()),
        tag: None,
    });
    Ok(ops)
}
