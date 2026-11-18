//! Database, configuration and layout infrastructure. Application composition
//! lives in crate::app; mutation-capable infrastructure is internal.
pub mod config;
pub(crate) mod db;
pub mod layout;
pub(crate) mod registry;
pub(crate) mod state;
