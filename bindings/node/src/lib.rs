//! Node.js binding over velo-core's embedder API.
//!
//! velo-core is synchronous, so every call runs on the libuv thread pool and
//! hands back a `Promise`; the event loop is never blocked.

mod errors;
mod history;
mod merge;
mod repo;
