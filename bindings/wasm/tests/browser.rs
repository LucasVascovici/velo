//! Runs in a real browser (`wasm-pack test --headless --chrome bindings/wasm
//! -- --test browser`), in a dedicated worker: OPFS synchronous access handles,
//! which the `sahpool` VFS needs, exist only there.
#![cfg(target_arch = "wasm32")]

mod common;

use common::*;
use velody_wasm::Repo;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_dedicated_worker);

#[wasm_bindgen_test]
async fn cookbook_scenario_persists_and_reopens() {
    // OPFS outlives the test run, so every run needs a name of its own.
    let name = format!("cookbook-{}.db", js_sys::Date::now() as u64);
    let repo = Repo::open_persistent(name.clone()).await.unwrap();
    let outcome = scenario(&repo);
    check_history(&repo, &outcome);
    drop(repo);

    let reopened = Repo::open_persistent(name).await.unwrap();
    check_history(&reopened, &outcome);
    check_fsck(&reopened);
}
