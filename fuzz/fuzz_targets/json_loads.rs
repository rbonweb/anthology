//! Fuzz target `json_loads` (F2, design §13.5): `pycompat::json` never panics on any input,
//! prints every parsed value under every GC-4 preset and as JavaScript, and a value without
//! NaN or Infinity reads back equal from each preset's text.
#![no_main]

use anthology_pycompat::json::{
    PRESETS, Value, dumps, js_stringify, loads, loads_bytes_lossy, loads_utf8_sig,
};

/// Whether `v` holds no NaN or infinite float (those print as `NaN`/`Infinity`, and NaN never
/// compares equal).
fn finite(v: &Value) -> bool {
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        match v {
            Value::Float(f) if !f.is_finite() => return false,
            Value::Array(a) => stack.extend(a),
            Value::Object(m) => stack.extend(m.iter().map(|(_, v)| v)),
            _ => {}
        }
    }
    true
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = loads_utf8_sig(data);
    let Ok(v) = loads_bytes_lossy(data) else {
        return;
    };
    let round_trips = finite(&v);
    for (name, o) in PRESETS {
        let text = dumps(&v, &o);
        if round_trips {
            let back = loads(&text).unwrap_or_else(|e| panic!("{name}: {e}: {text}"));
            assert!(back == v, "{name}: {text}");
        }
    }
    let js = js_stringify(&v);
    assert!(loads(&js).is_ok(), "{js}");
    if round_trips {
        assert!(v.clone() == v);
    }
});
