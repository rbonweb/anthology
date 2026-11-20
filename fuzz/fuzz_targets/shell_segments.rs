//! Fuzz target `shell_segments` (stub until its lane fills it).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = data;
});
