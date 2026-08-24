pub mod bus;
pub mod data;
pub mod instance;
pub mod java;
pub mod utils;
pub mod version;

pub use data::{config::Config, AppError, GlobalState};

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GlobalState>();
};
