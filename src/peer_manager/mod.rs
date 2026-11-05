mod peer;

use crate::{
    downloader::METAINFO,
    error::Error,
    metainfo::FileDesc,
    peer_manager::peer::{Peer, PeerError},
};
use futures::FutureExt;
use sha1::{Digest, Sha1};
use std::{
    collections::HashSet,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncSeekExt, AsyncWriteExt},
    sync::{
        RwLock,
        mpsc::{self, Receiver, error::TryRecvError},
    },
    task::JoinHandle,
    time,
};

const MAX_CONNECTIONS: usize = 100;

#[derive(Debug, Clone)]
struct PieceRequest {
    index: usize,
    // hash: [u8; 20],
    availability: Arc<AtomicUsize>,
    reserved: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct PieceResponse {
    index: usize,
    buf: Vec<u8>,
}

pub struct MinHeap(Vec<PieceRequest>);

struct OutputFile {
    file: File,
    start: u64,
    end: u64,
}

struct OutputFiles(Vec<OutputFile>);

impl OutputFiles {
    async fn create(root: &Path, base: &Path, files: &[FileDesc]) -> Result<Self, Error> {
        let base = safe_relative_path(base)?;
        let mut output_files = Vec::with_capacity(files.len());
        let mut start = 0u64;

        for desc in files {
            let relative_path = safe_relative_path(&base.join(&desc.path))?;
            if relative_path.as_os_str().is_empty() {
                return Err(Error::InvalidField("Empty torrent output path".into()));
            }
            let length = u64::try_from(desc.length).map_err(|_| {
                Error::InvalidField(format!("Invalid negative file length: {}", desc.length))
            })?;
            let end = start
                .checked_add(length)
                .ok_or_else(|| Error::InvalidField("Total file length overflows u64".into()))?;
            let path = root.join(relative_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await.map_err(Error::WriteFile)?;
            }
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)
                .await
                .map_err(Error::WriteFile)?;
            file.set_len(length).await.map_err(Error::WriteFile)?;
            output_files.push(OutputFile { file, start, end });
            start = end;
        }

        Ok(Self(output_files))
    }

    async fn write_piece(&mut self, offset: u64, bytes: &[u8]) -> Result<(), Error> {
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| Error::InvalidField("Piece write range overflows u64".into()))?;

        for output in &mut self.0 {
            let overlap_start = offset.max(output.start);
            let overlap_end = end.min(output.end);
            if overlap_start >= overlap_end {
                continue;
            }

            let piece_start = usize::try_from(overlap_start - offset)
                .map_err(|_| Error::Internal("Piece offset exceeds platform limits".into()))?;
            let piece_end = usize::try_from(overlap_end - offset)
                .map_err(|_| Error::Internal("Piece offset exceeds platform limits".into()))?;
            output
                .file
                .seek(std::io::SeekFrom::Start(overlap_start - output.start))
                .await
                .map_err(Error::WriteFile)?;
            output
                .file
                .write_all(&bytes[piece_start..piece_end])
                .await
                .map_err(Error::WriteFile)?;
            output.file.flush().await.map_err(Error::WriteFile)?;
        }

        Ok(())
    }
}

fn safe_relative_path(path: &Path) -> Result<PathBuf, Error> {
    let mut safe_path = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => safe_path.push(part),
            Component::CurDir => {}
            _ => {
                return Err(Error::InvalidField(format!(
                    "Unsafe torrent output path: {}",
                    path.display()
                )));
            }
        }
    }
    Ok(safe_path)
}

impl MinHeap {
    unsafe fn from_raw_vec(v: Vec<PieceRequest>) -> Self {
        Self(v)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn reserve(&self, available: &[bool]) -> Option<usize> {
        let mut candidates: Vec<_> = self
            .0
            .iter()
            .filter(|piece| available.get(piece.index) == Some(&true))
            .collect();
        candidates.sort_unstable_by_key(|piece| piece.availability.load(Ordering::Relaxed));

        candidates.into_iter().find_map(|piece| {
            piece
                .reserved
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                .ok()
                .map(|_| piece.index)
        })
    }

    pub fn release(&self, index: usize) {
        if let Some(piece) = self.0.iter().find(|piece| piece.index == index) {
            piece.reserved.store(false, Ordering::Release);
        }
    }

    fn remove(&mut self, index: usize) {
        self.0.retain(|piece| piece.index != index);
    }

    pub fn change_element(&self, index: usize, change: i64) {
        let Some(piece) = self.0.iter().find(|piece| piece.index == index) else {
            return;
        };
        let change_result = if change >= 0 {
            piece
                .availability
                .try_update(Ordering::AcqRel, Ordering::Relaxed, |availability| {
                    availability.checked_add(change as usize)
                })
        } else {
            piece
                .availability
                .try_update(Ordering::AcqRel, Ordering::Relaxed, |availability| {
                    availability.checked_sub(change.unsigned_abs() as usize)
                })
        };
        if change_result.is_err() {
            log::error!("piece availability counter would overflow or underflow (id={index})");
        }
    }
}

pub async fn run(
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    mut other_peer_rx: Receiver<SocketAddr>,
) -> Result<(), Error> {
    let pieces: Vec<PieceRequest> = METAINFO
        .info
        .pieces
        .iter()
        .enumerate()
        .map(|(index, _)| PieceRequest {
            index,
            availability: Arc::new(AtomicUsize::new(0)),
            reserved: Arc::new(AtomicBool::new(false)),
        })
        .collect();

    // SAFETY: since we start, availability is 0 for all pieces, thus it's ordered.
    let pieces = unsafe { MinHeap::from_raw_vec(pieces) };
    let required_pieces = Arc::new(RwLock::new(pieces));
    let mut output_files = OutputFiles::create(
        Path::new("."),
        &METAINFO.info.files.path,
        &METAINFO.info.files.files,
    )
    .await?;

    let (pieces_tx, mut pieces_rx) = mpsc::unbounded_channel::<PieceResponse>();

    let mut connections: Vec<(SocketAddr, JoinHandle<Result<(), PeerError>>)> = Vec::new();
    let mut connections_addrs = HashSet::new();

    loop {
        connections.retain_mut(|(addr, task)| match task.now_or_never() {
            None => true,
            Some(Ok(Ok(()))) => {
                connections_addrs.remove(addr);
                false
            }
            Some(Ok(Err(error))) => {
                log::debug!(
                    "peer (id={}, addr={}) exited with error: {}",
                    error.index,
                    addr,
                    error.inner
                );
                connections_addrs.remove(addr);
                false
            }
            Some(Err(error)) => {
                log::debug!("peer task failed: {error}");
                connections_addrs.remove(addr);
                false
            }
        });

        if connections.len() < MAX_CONNECTIONS {
            for _ in 0..(MAX_CONNECTIONS - connections.len()) {
                let other_addr = match other_peer_rx.try_recv() {
                    Ok(a) => a,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        unreachable!("we hold another tx in this thread") // todo: false
                    }
                };

                if !connections_addrs.insert(other_addr) {
                    continue;
                }

                let peer_info = Peer {
                    index: connections.len(),
                    addr: other_addr,
                    peer_id,
                    info_hash,
                    tx: pieces_tx.clone(),
                    required_pieces: Arc::clone(&required_pieces),
                };
                let peer = tokio::spawn(Peer::run(peer_info));
                connections.push((other_addr, peer));
            }
        }

        let piece = time::timeout(Duration::from_millis(50), pieces_rx.recv()).await;
        let piece = match piece {
            Ok(Some(p)) => p,
            Ok(None) => {
                log::error!("got ok none!");
                continue;
            }
            Err(_) => continue,
        };

        let expected_hash = METAINFO.info.pieces[piece.index];

        let retrieved_hash = Sha1::digest(&piece.buf);
        if expected_hash.as_slice() != retrieved_hash.as_slice() {
            log::warn!("not correct piece (id={})", piece.index);
            required_pieces.write().await.release(piece.index);
            continue;
        }

        let piece_length = u64::try_from(METAINFO.info.piece_length)
            .map_err(|_| Error::InvalidField("Invalid piece length".into()))?;
        let offset = (piece.index as u64)
            .checked_mul(piece_length)
            .ok_or_else(|| Error::InvalidField("Piece offset overflows u64".into()))?;
        output_files.write_piece(offset, &piece.buf).await?;

        log::info!("yes, received it (id={}!", piece.index);
        required_pieces.write().await.remove(piece.index);

        if required_pieces.read().await.is_empty() {
            break; // all received
        }
    }

    log::info!("all received!");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::OutputFiles;
    use crate::metainfo::FileDesc;
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    #[tokio::test]
    async fn writes_piece_bytes_across_file_boundaries() {
        let root = std::env::temp_dir().join(format!(
            "got-piece-output-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let files = [
            FileDesc {
                length: 3,
                md5sum: None,
                path: PathBuf::from("first"),
            },
            FileDesc {
                length: 4,
                md5sum: None,
                path: PathBuf::from("nested/second"),
            },
        ];

        let mut output = OutputFiles::create(&root, Path::new("torrent"), &files)
            .await
            .unwrap();
        output.write_piece(1, b"abcdef").await.unwrap();
        drop(output);

        assert_eq!(fs::read(root.join("torrent/first")).unwrap(), b"\0ab");
        assert_eq!(
            fs::read(root.join("torrent/nested/second")).unwrap(),
            b"cdef"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn writes_single_file_torrent_with_current_directory_root() {
        let root = std::env::temp_dir().join(format!(
            "got-single-file-output-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let files = [FileDesc {
            length: 4,
            md5sum: None,
            path: PathBuf::from("payload.bin"),
        }];

        let mut output = OutputFiles::create(&root, Path::new("./"), &files)
            .await
            .unwrap();
        output.write_piece(0, b"data").await.unwrap();
        drop(output);

        assert_eq!(fs::read(root.join("payload.bin")).unwrap(), b"data");
        fs::remove_dir_all(root).unwrap();
    }
}
