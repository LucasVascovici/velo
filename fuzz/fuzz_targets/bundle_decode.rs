#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    velody_core::fuzzing::bundle_decode(data);
});
