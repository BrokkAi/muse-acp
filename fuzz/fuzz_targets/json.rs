#![no_main]
#[path = "../invariants.rs"]
mod invariants;
#[allow(dead_code)]
#[path = "../../src/json.rs"]
mod json;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    invariants::check(data);
});
