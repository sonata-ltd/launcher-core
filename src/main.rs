use std::process::exit;

use crate::cli::run;

mod cli;

#[async_std::main]
async fn main() {
    tracing_subscriber::fmt::init();

    if let Err(e) = run().await {
        tracing::error!("{e}");
        exit(1);
    }
}
