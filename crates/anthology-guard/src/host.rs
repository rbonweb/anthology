//! Where the guard runs: a person's terminal, or a program that embeds Claude Code (an SDK host).
//!
//! A host has no `/plugin` command and nobody to type it, so a message that sends the reader there
//! (the `(G0)` line's `/plugin disable` aside) is dropped for it. This is text only: no verdict, exit
//! code or rule depends on it.
//!
//! The module is not re-exported at the crate root; call `anthology_guard::host::embedded`.

/// The environment variable Claude Code sets to say how it was started (`cli`, `claude-vscode`,
/// `sdk-cli`, `sdk-ts`, ...). A host may preset a name of its own, which Claude Code keeps.
pub const ENTRYPOINT_VAR: &str = "CLAUDE_CODE_ENTRYPOINT";

/// The environment variable Claude Code sets to `1` in an interactive session and to `0` for `-p` and
/// the SDK. It recomputes the value even when one is preset, so a host cannot hide behind a custom
/// entrypoint.
pub const ATTENDED_VAR: &str = "CLAUDE_CODE_SESSION_ATTENDED";

/// The environment variable an operator sets to `full` to give an embedded host the full terminal
/// experience.
pub const MODE_VAR: &str = "ANTHOLOGY_HOST_MODE";

/// The value of [`MODE_VAR`] that turns the embedded reading off.
const FULL: &str = "full";

/// Whether the guard runs inside an embedding host: `mode` (the value of [`MODE_VAR`]) is not exactly
/// `full`, and either `attended` (the value of [`ATTENDED_VAR`]) is exactly `0` or `entrypoint` (the
/// value of [`ENTRYPOINT_VAR`]) starts with `sdk-`. An unset variable is `None`; an empty one is
/// `Some("")`, which matches nothing.
pub fn embedded(entrypoint: Option<&str>, attended: Option<&str>, mode: Option<&str>) -> bool {
    mode != Some(FULL)
        && (attended == Some("0") || entrypoint.is_some_and(|e| e.starts_with("sdk-")))
}

/// [`embedded`] for this process: the three variables from its environment. A variable that is unset or
/// not valid UTF-8 counts as unset.
pub fn embedded_from_env() -> bool {
    let var = |name: &str| std::env::var(name).ok();
    embedded(
        var(ENTRYPOINT_VAR).as_deref(),
        var(ATTENDED_VAR).as_deref(),
        var(MODE_VAR).as_deref(),
    )
}
