#![feature(random)]

mod args;
mod bencoding;
mod downloader;
mod error;
mod metainfo;
mod peer_manager;
mod url;

use crate::downloader::Downloader;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    pretty_env_logger::init();

    let Some(target) = args::parse_args() else {
        return ExitCode::from(1);
    };

    if let Err(e) = Downloader::download(target).await {
        log::error!("{e}");
        return ExitCode::from(1);
    }

    ExitCode::from(0)
}
