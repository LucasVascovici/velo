//! Minimised inputs for crashes found by the `cargo fuzz` targets in `fuzz/`.
//!
//! Each test feeds the input to the same `pub(crate)` function the fuzz wrapper
//! calls, without the `fuzzing` feature, and asserts only that it returns. No
//! crash has been found yet, so the one test here is a sweep that keeps the
//! seed corpus honest: every prefix of every golden bundle, and every byte
//! flipped in the smallest, must be refused or accepted, never panic.

use std::fs;
use std::path::Path;

use crate::commands::bundle;
use crate::{transport, Limits};

fn tight() -> Limits {
    Limits::default()
        .with_max_object_bytes(16 << 20)
        .with_max_pack_bytes(64 << 20)
        .with_max_string_bytes(1 << 20)
}

#[test]
fn truncated_and_flipped_corpus_bundles_never_panic() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/bundle_decode");
    let mut smallest: Option<Vec<u8>> = None;
    for entry in fs::read_dir(dir).unwrap() {
        let data = fs::read(entry.unwrap().path()).unwrap();
        for len in 0..data.len().min(4096) {
            let _ = bundle::decode(&data[..len], &tight());
        }
        if smallest.as_ref().is_none_or(|s| data.len() < s.len()) {
            smallest = Some(data);
        }
    }
    let data = smallest.expect("seed bundles");
    for i in 0..data.len() {
        let mut copy = data.clone();
        copy[i] ^= 0xff;
        let _ = bundle::decode(&copy, &tight());
    }
}

#[test]
fn wire_readers_return_on_every_prefix_of_a_hostile_stream() {
    let mut stream = Vec::new();
    stream.extend_from_slice(&u32::MAX.to_le_bytes());
    stream.extend_from_slice(&u32::MAX.to_le_bytes());
    stream.extend_from_slice(b"main");
    for len in 0..=stream.len() {
        let mut r = &stream[..len];
        let _ = transport::read_refs(&mut r);
        let mut r = &stream[..len];
        while let Ok(Some(_)) = transport::read_string_opt(&mut r, 1 << 20) {}
    }
}
