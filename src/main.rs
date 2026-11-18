use std::process::exit;

use crate::cli::run;

mod cli;

#[async_std::main]
async fn main() {
    tracing_subscriber::fmt::init();

    if let Err(e) = run().await {
        if e.is::<cli::ParseError>() {
            eprintln!("{e}");
            exit(2);
        }
        tracing::error!("{e}");
        exit(1);
    }
}
