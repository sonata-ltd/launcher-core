//! Database, configuration and layout infrastructure. Application composition
//! lives in crate::app; old data imports remain compatibility aliases until P11.
pub mod config;
pub mod db;
pub mod layout;
pub mod registry;
pub mod state;

pub use crate::app::{AppError, GlobalDataStateResult, GlobalState, LauncherServices};
