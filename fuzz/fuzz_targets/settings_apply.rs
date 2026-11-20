//! Fuzz target `settings_apply` (stub until its lane fills it).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = data;
});
