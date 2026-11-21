//! `eval-ceiling` (U9): `--max-cost-usd` stops the run after the first of two runs, as a partial
//! result (exit 2).

use std::fs;

use anthology_pycompat::json::{self, Value};

use crate::smoke::runner::*;

const CASE_YAML: &str = "schema_version: \"1.0\"\nname: ceiling\ndescription: cost ceiling probe\n\
plugins: [\"../../plugins/smoke-probe\"]\n\
execution:\n  prompt: \"Reply with the single word OK.\"\n  model: haiku\n  max_turns: 1\n\
runs: 2\ngraders:\n  - name: said-ok\n    type: regex\n    target: last_message\n    pattern: 'OK'\n";

/// `len(x)` of a JSON value, `None` where Python's `len` raises `TypeError`.
fn py_len(v: &Value) -> Option<usize> {
    match v {
        Value::Str(s) => Some(s.chars().count()),
        Value::Array(a) => Some(a.len()),
        Value::Object(o) => Some(o.len()),
        _ => None,
    }
}

/// `len(res["cases"][0]["arms"]["with"])`, `None` for any KeyError, IndexError or TypeError.
fn recorded_runs(res: &Value) -> Option<usize> {
    let case = field(res, "cases")?.as_array()?.first()?;
    py_len(field(field(case, "arms")?, "with")?)
}

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let tmp = scratch()?;
    let d = tmp.join("ws");
    write_plugin(
        &d.join("plugins").join("smoke-probe"),
        "smoke-probe",
        None,
        &[],
    )?;
    mkdirs(&d.join(".claude-plugin"))?;
    let marketplace = jobj(vec![
        ("name", jstr("smoke")),
        ("owner", jobj(vec![("name", jstr("smoke"))])),
        (
            "plugins",
            jarr(vec![jobj(vec![
                ("name", jstr("smoke-probe")),
                ("source", jstr("./plugins/smoke-probe")),
            ])]),
        ),
    ]);
    write_text(
        &d.join(".claude-plugin/marketplace.json"),
        &dumps(&marketplace),
    )?;
    mkdirs(&d.join("evals").join("ceiling"))?;
    write_text(&d.join("evals/ceiling/case.yaml"), CASE_YAML)?;
    let args = sv([
        "plugin",
        "eval",
        ".",
        "--trust-plugin",
        "--ablation",
        "none",
        "--no-publish",
        "--max-cost-usd",
        "0.000001",
        "--runs",
        "2",
        "--json",
        "out.json",
    ]);
    let opts = ClaudeOpts {
        cwd: Some(d.clone()),
        extra_env: Some(no_scrub()),
        timeout: 600,
    };
    let p = ctx.claude_run(&tmp, &args, &opts);
    let loaded = fs::read_to_string(d.join("out.json"))
        .ok()
        .and_then(|text| json::loads(&text).ok());
    let Some((res, runs)) = loaded.and_then(|res| {
        let runs = recorded_runs(&res)?;
        Some((res, runs))
    }) else {
        return Ok((
            Status::Fail,
            format!(
                "no usable result JSON (exit {}): {}",
                p.code,
                ctx.snippet(&p.text(), 160)
            ),
        ));
    };
    ctx.add_spent(number_field(&res, "costUsd").unwrap_or(0.0));
    let mut bad: Vec<String> = Vec::new();
    if p.code != 2 {
        bad.push(format!("exit {}, want 2", p.code));
    }
    let partial = field(&res, "partial");
    let reason = field(&res, "partialReason");
    if partial != Some(&Value::Bool(true)) || reason.and_then(Value::as_str) != Some("cost_ceiling")
    {
        let repr = |v: Option<&Value>| v.map_or_else(|| "None".to_string(), py_repr);
        bad.push(format!("partial={} reason={}", repr(partial), repr(reason)));
    }
    if runs != 1 {
        bad.push(format!("{runs} runs recorded, want 1"));
    }
    Ok(if bad.is_empty() {
        (
            Status::Pass,
            "stopped at the cost ceiling after 1 of 2 runs (partial, exit 2)".to_string(),
        )
    } else {
        (Status::Fail, bad.join("; "))
    })
}
