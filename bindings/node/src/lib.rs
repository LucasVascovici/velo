//! Node.js binding over velody-core's embedder API.
//!
//! velody-core is synchronous, so every call runs on the libuv thread pool and
//! hands back a `Promise`; the event loop is never blocked.

mod callbacks;
mod errors;
mod history;
mod merge;
mod repo;
mod signing;
mod store;
