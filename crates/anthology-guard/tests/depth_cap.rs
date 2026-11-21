//! Final review I-1: text nested past anthology-shell's depth cap (`Opaque::DepthCap`) was never
//! analyzed, so the verdict is [`Verdict::TooComplex`] (G0), whatever it hides. The shipped Python
//! allows these (D-8, Rust stricter: `port/difftest/divergences/guard.check.toml`).

#[path = "support/gd.rs"]
mod gd;

use anthology_guard::{Rule, Verdict, check, check_without_d8};

use gd::{HOME, bash};

fn verdict(command: &str) -> Verdict {
    let (tool, input) = bash(command);
    check(&tool, &input, HOME)
}

/// `levels` nested `$(echo …)` around `npx playwright install`.
fn nested_substitutions(levels: usize) -> String {
    let mut c = "npx playwright install".to_string();
    for _ in 0..levels {
        c = format!("echo $({c})");
    }
    c
}

/// `levels` `eval`s before `npx playwright install`.
fn evals(levels: usize) -> String {
    format!("{}npx playwright install", "eval ".repeat(levels))
}

#[test]
fn thirteen_nested_substitutions_are_too_complex() {
    // one level less is analyzed to the end, and G1 blocks it
    assert_eq!(verdict(&nested_substitutions(12)), Verdict::Block(Rule::G1));
    assert_eq!(verdict(&nested_substitutions(13)), Verdict::TooComplex);
    assert_eq!(verdict(&nested_substitutions(40)), Verdict::TooComplex);
}

#[test]
fn thirteen_evals_are_too_complex() {
    assert_eq!(verdict(&evals(12)), Verdict::Block(Rule::G1));
    assert_eq!(verdict(&evals(13)), Verdict::TooComplex);
    assert_eq!(verdict(&evals(40)), Verdict::TooComplex);
}

#[test]
fn a_depth_capped_benign_command_is_too_complex_too() {
    let c = format!("{}echo hi", "eval ".repeat(13));
    assert_eq!(verdict(&c), Verdict::TooComplex);
    assert_eq!(verdict("eval eval echo hi"), Verdict::Allow);
}

#[test]
fn the_depth_cap_block_is_a_d8_part() {
    // `check_without_d8` leaves it out (the difftest generator keeps D-8 case-scoped)
    let (tool, input) = bash(&evals(13));
    assert_eq!(check_without_d8(&tool, &input, HOME), Verdict::Allow);
}
