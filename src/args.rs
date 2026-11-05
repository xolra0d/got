use std::{env, path};

const HELP_MESSAGE: &str = "CLI Torrent downloader.

Usage:
got torrentfile.torrent # start downloading from file
got --help # print this message";
// got magnet:?... # start downloading from magnet link

pub enum Target {
    // MagnetLink(String),
    TorrentFile(path::PathBuf),
}

pub fn parse_args() -> Option<Target> {
    match env::args_os().nth(1) {
        None => {
            log::error!("No target was provided! Use \"--help\".");
            println!("{HELP_MESSAGE}");
            None
        }
        Some(h) if h == "--help" => {
            println!("{HELP_MESSAGE}");
            None
        }
        Some(path) => Some(Target::TorrentFile(path::PathBuf::from(path))),
    }
}
