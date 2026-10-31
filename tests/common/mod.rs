//! Shared fixtures for the integration tests.
//!
//! The artifacts under `demo/` are the real published ones: a signed log, the rules document it
//! was decided under, and an endorsement from a second key. They are read, never written, so the
//! tests check the same bytes a visitor to the demonstration site would fetch.
#![allow(dead_code)]

use std::path::PathBuf;

pub fn demo_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("demo").join(name)
}

pub fn read_json(path: &PathBuf) -> serde_json::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
}

/// A signing key in a throwaway file under the OS temp directory. Never the real key location:
/// these tests must not create or touch the runtime's key, and must not depend on it existing.
pub fn temp_key_path(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join(format!("levos-it-{tag}-{}-{nanos}.key", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

pub fn sealer(tag: &str) -> levos::receipt::Sealer {
    let path = temp_key_path(tag);
    levos::receipt::Sealer::load_or_create(&path).expect("a temp sealing key")
}
