mod udp;

use crate::{
    args::Target,
    bencoding::{ToBencode, from_bencode},
    error::Error,
    metainfo::MetaInfo,
    peer_manager,
    url::UrlScheme,
};
use sha1::{Digest, Sha1};
use std::{
    borrow::Borrow,
    marker::PhantomPinned,
    mem::ManuallyDrop,
    ops::Deref,
    path,
    pin::Pin,
    random::{Rng, SystemRng},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicI32, AtomicI64},
    },
};
use tokio::{
    fs,
    sync::{Notify, mpsc},
};

pub struct Downloader;

struct MetaRefInner {
    meta: ManuallyDrop<Option<MetaInfo<'static>>>,
    bytes: ManuallyDrop<Vec<u8>>,
    _pin: PhantomPinned,
}

impl Drop for MetaRefInner {
    fn drop(&mut self) {
        let Self { meta, bytes, _pin } = self;
        unsafe {
            ManuallyDrop::drop(meta);
            ManuallyDrop::drop(bytes);
        }
    }
}

impl MetaRefInner {
    fn new(bytes: Vec<u8>) -> Result<Pin<Box<Self>>, Error> {
        let mut boxed = Box::pin(Self {
            bytes: ManuallyDrop::new(bytes),
            meta: ManuallyDrop::new(None),
            _pin: PhantomPinned,
        });

        let meta =
            from_bencode(unsafe { &*(boxed.bytes.as_slice() as *const [u8]) })?.as_dictionary()?;
        let meta = MetaInfo::new(meta)?;

        unsafe {
            *boxed.as_mut().get_unchecked_mut().meta = Some(meta);
        }
        Ok(boxed)
    }

    fn view(&self) -> &MetaInfo<'static> {
        self.meta.as_ref().unwrap()
    }
}

pub struct MetaRef {
    meta: OnceLock<Pin<Box<MetaRefInner>>>,
}

pub static METAINFO: MetaRef = MetaRef {
    meta: OnceLock::new(),
};

impl MetaRef {
    fn init(&self, bytes: Vec<u8>) -> Result<(), Error> {
        let inner = MetaRefInner::new(bytes)?;
        self.meta.get_or_init(|| inner);

        Ok(())
    }
}

impl Borrow<MetaInfo<'static>> for MetaRef {
    fn borrow(&self) -> &MetaInfo<'static> {
        self.meta.get().unwrap().view()
    }
}

impl Deref for MetaRef {
    type Target = MetaInfo<'static>;

    fn deref(&self) -> &Self::Target {
        self.meta.get().unwrap().view()
    }
}

impl Downloader {
    pub async fn download(target: Target) -> Result<(), Error> {
        let mut peer_id = [0u8; 20];
        SystemRng.fill_bytes(&mut peer_id);

        match target {
            Target::TorrentFile(path) => Self::download_torrent_file(path, peer_id).await,
        }
    }

    pub async fn download_torrent_file(
        path: path::PathBuf,
        peer_id: [u8; 20],
    ) -> Result<(), Error> {
        let file = fs::read(path).await.map_err(Error::ReadFile)?;

        METAINFO.init(file)?;

        let force_announce = Arc::new(Notify::new());
        let downloaded = AtomicI64::new(0);
        let uploaded = AtomicI64::new(0);
        let event = AtomicI32::new(0);
        let (tx, rx) = mpsc::channel(100);

        let tracker_comm = match &METAINFO.announce.scheme() {
            UrlScheme::Udp => tokio::spawn(udp::start_announce(
                peer_id,
                tx,
                downloaded,
                uploaded,
                event,
                Arc::clone(&force_announce),
            )),
        };

        let peer_manager = tokio::spawn(peer_manager::run(
            info_hash(&METAINFO).expect("TODO"),
            peer_id,
            rx,
        ));

        let peer_result = peer_manager
            .await
            .map_err(|error| Error::Internal(format!("Peer manager task failed: {error}")));

        tracker_comm.abort();
        match tracker_comm.await {
            Ok(result) => result?,
            Err(error) if error.is_cancelled() => {}
            Err(error) => {
                return Err(Error::Internal(format!("Tracker task failed: {error}")));
            }
        }

        peer_result??;

        Ok(())
    }
}

pub fn info_hash(meta: &MetaInfo<'_>) -> Result<[u8; 20], Error> {
    let mut encoded = Vec::new();
    meta.raw_info
        .to_bencode(&mut encoded)
        .map_err(|e| Error::Internal(format!("Failed to encode info dictionary: {e}")))?;
    Ok(Sha1::digest(encoded).into())
}

pub fn total_length(meta: &MetaInfo<'_>) -> Result<i64, Error> {
    if let Some(length) = meta.raw_info.get(b"length".as_slice()) {
        return length.clone().as_integer().map_err(Error::from);
    }

    let files = meta
        .raw_info
        .get(b"files".as_slice())
        .ok_or_else(|| Error::MissingField(String::from("length or files")))?;
    let files = files.clone().as_list()?;
    files.into_iter().try_fold(0i64, |total, file| {
        let file = file.as_dictionary()?;
        let length = file
            .get(b"length".as_slice())
            .ok_or_else(|| Error::MissingField(String::from("files[].length")))?
            .clone()
            .as_integer()?;
        total
            .checked_add(length)
            .ok_or_else(|| Error::InvalidField(String::from("Total file length overflows i64")))
    })
}
