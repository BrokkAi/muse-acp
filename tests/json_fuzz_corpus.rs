#[path = "../fuzz/invariants.rs"]
mod invariants;
#[allow(dead_code)]
#[path = "../src/json.rs"]
mod json;

#[test]
fn fuzz_seed_corpus_preserves_parser_invariants() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/seeds/json");
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        invariants::check(&std::fs::read(entry.path()).unwrap());
    }
}
