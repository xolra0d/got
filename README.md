got
---

CLI-based torrent client.

## Build

Try it out: 
```bash
cargo build --release 
cargo run --release -- --help
cargo run --release -- big-buck-bunny.torrent
# RUST_LOG=debug cargo run --release big-buck-bunny.torrent # to run with debug logs
```

As of now supports:
- BEP0003
- BEP0015
