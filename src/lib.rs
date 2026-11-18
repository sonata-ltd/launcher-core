//! Supported consumer API is service-oriented. Infrastructure capabilities
//! cannot be obtained through observer DTOs or a launcher handle.
//!
//! SQL access is not part of the public API:
//! ```compile_fail
//! use sonata_launcher_core::data::db::Database;
//! ```
//! ```compile_fail
//! fn bypass(core: &sonata_launcher_core::GlobalState) { let _ = &core.db.pool; }
//! ```
//! ```compile_fail
//! fn bypass(core: &sonata_launcher_core::GlobalState) { let _ = &core.services; }
//! ```
//! Registry/admission mutation is owned by services:
//! ```compile_fail
//! use sonata_launcher_core::data::registry::operation::OperationRegistry;
//! ```
//! ```compile_fail
//! fn bypass(core: &sonata_launcher_core::GlobalState) { let _ = &core.operations; }
//! ```
//! ```compile_fail
//! use sonata_launcher_core::instance::registry::InstanceRegistry;
//! ```
//! ```compile_fail
//! use sonata_launcher_core::java::registry::JavaRegistry;
//! ```
//! ```compile_fail
//! use sonata_launcher_core::instance::storage::DbInstanceStore;
//! ```
//! ```compile_fail
//! use sonata_launcher_core::java::storage::DbJavaStore;
//! ```
//! ```compile_fail
//! use sonata_launcher_core::instance::content::storage;
//! ```
//! ```compile_fail
//! use sonata_launcher_core::instance::content::install;
//! ```
//! ```compile_fail
//! use sonata_launcher_core::instance::content::recovery;
//! ```
//! ```compile_fail
//! use sonata_launcher_core::instance::init::install;
//! ```
//! ```compile_fail
//! use sonata_launcher_core::instance::launch::resolved;
//! ```
//! ```compile_fail
//! use sonata_launcher_core::modloader::repository::Repository;
//! ```
//! ```compile_fail
//! use sonata_launcher_core::modloader::forge::processor;
//! ```
//! Composition aliases and the old managed validation shim are removed:
//! ```compile_fail
//! use sonata_launcher_core::data::GlobalState;
//! ```
//! ```compile_fail
//! async fn old(java: &sonata_launcher_core::java::service::JavaService, id: uuid::Uuid) {
//!     java.managed.validate(id, java).await;
//! }
//! ```
pub mod app;
pub mod bus;
pub mod data;
pub mod instance;
pub mod java;
pub mod modloader;
pub mod operations;
pub mod utils;
pub mod version;

pub use app::{AppError, GlobalState};
pub use data::config::Config;
pub use data::db::DbError;

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GlobalState>();
};
