pub mod app;
pub mod bus;
pub mod data;
pub mod instance;
pub mod java;
pub mod modloader;
pub mod utils;
pub mod version;

pub use app::{AppError, GlobalState};
pub use data::config::Config;

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GlobalState>();
};
