//! Configuration storage: on-disk model, directory location and CRUD.
//!
//! The on-disk format is the ssr-n JSON schema from `ARCHITECTURE.md` §6.3 —
//! it is an **external contract**: `ssr-client-rs` parses the same files, so
//! field names/structure must not change.

pub mod model;
pub mod path;
pub mod store;
