//! The in-memory variant of the cookbook scenario in a real browser (a
//! dedicated worker), without persistence.
#![cfg(target_arch = "wasm32")]

mod common;

use common::*;
use velody_wasm::Repo;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_dedicated_worker);

#[wasm_bindgen_test]
fn cookbook_scenario_in_memory() {
    let repo = Repo::create_in_memory("cookbook-memory.db").unwrap();
    let outcome = scenario(&repo);
    check_history(&repo, &outcome);
}
