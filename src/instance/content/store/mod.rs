// Compatibility facade: the byte store has no dependency on content/SQLite.
pub use crate::utils::object_store::{ObjectStore, StoredObject};

pub(in crate::instance::content) mod legacy;
pub mod storage;
