use crate::{
    downloader::{METAINFO, total_length},
    peer_manager::{MinHeap, PieceResponse},
};
use std::{collections::HashMap, fmt::Display, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{RwLock, mpsc::UnboundedSender},
    time,
};

const BLOCK_LENGTH: usize = 16 * 1024;
const MAX_OUTSTANDING_REQUESTS: usize = 5;
const MAX_MESSAGE_LENGTH: usize = 16 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

enum Message {
    Interested,
    Request {
        index: usize,
        begin: usize,
        length: usize,
    },
}

impl Message {
    fn to_bytes(&self) -> Option<Vec<u8>> {
        let (id, payload) = match self {
            Self::Interested => (2, Vec::new()),
            Self::Request {
                index,
                begin,
                length,
            } => {
                let mut payload = Vec::with_capacity(12);
                payload.extend_from_slice(&u32::try_from(*index).ok()?.to_be_bytes());
                payload.extend_from_slice(&u32::try_from(*begin).ok()?.to_be_bytes());
                payload.extend_from_slice(&u32::try_from(*length).ok()?.to_be_bytes());
                (6, payload)
            }
        };

        let length = 1 + payload.len();
        let mut bytes = Vec::with_capacity(4 + length);
        bytes.extend_from_slice(&(length as u32).to_be_bytes());
        bytes.push(id);
        bytes.extend_from_slice(&payload);
        Some(bytes)
    }
}

struct PendingPiece {
    index: usize,
    data: Vec<u8>,
    next_begin: usize,
    outstanding: HashMap<usize, usize>,
    received: usize,
}

impl PendingPiece {
    fn new(index: usize, length: usize) -> Self {
        Self {
            index,
            data: vec![0; length],
            next_begin: 0,
            outstanding: HashMap::new(),
            received: 0,
        }
    }
}

pub struct Peer {
    pub index: usize,
    pub addr: SocketAddr,
    pub peer_id: [u8; 20],
    pub info_hash: [u8; 20],
    pub tx: UnboundedSender<PieceResponse>,
    pub required_pieces: Arc<RwLock<MinHeap>>,
}

pub enum PeerErrorInner {
    BadPeer,
    ConnectError(io::Error),
    ReadError(io::Error),
    WriteError(io::Error),
    Internal(String),
}

impl Display for PeerErrorInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadPeer => f.write_str("bad peer"),
            Self::ConnectError(error) => write!(f, "failed to connect: {error}"),
            Self::ReadError(error) => write!(f, "failed to read: {error}"),
            Self::WriteError(error) => write!(f, "failed to write: {error}"),
            Self::Internal(error) => f.write_str(error),
        }
    }
}

pub struct PeerError {
    pub index: usize,
    pub inner: PeerErrorInner,
}

impl Peer {
    pub async fn run(mut self) -> Result<(), PeerError> {
        log::debug!("connecting to peer {}", self.addr);
        let mut stream = time::timeout(CONNECT_TIMEOUT, TcpStream::connect(self.addr))
            .await
            .map_err(|_| {
                self.error(PeerErrorInner::ConnectError(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("connection timed out after {CONNECT_TIMEOUT:?}"),
                )))
            })?
            .map_err(|error| self.error(PeerErrorInner::ConnectError(error)))?;
        self.handshake(&mut stream).await?;
        self.run_loop(&mut stream).await
    }

    async fn handshake(&mut self, stream: &mut TcpStream) -> Result<(), PeerError> {
        const PROTOCOL_NAME: &[u8; 19] = b"BitTorrent protocol";
        const SUPPORTED_EXTENSIONS: &[u8; 8] = &[0; 8];

        let mut handshake = Vec::with_capacity(68);
        handshake.push(PROTOCOL_NAME.len() as u8);
        handshake.extend_from_slice(PROTOCOL_NAME);
        handshake.extend_from_slice(SUPPORTED_EXTENSIONS);
        handshake.extend_from_slice(&self.info_hash);
        handshake.extend_from_slice(&self.peer_id);

        stream
            .write_all(&handshake)
            .await
            .map_err(|error| self.error(PeerErrorInner::WriteError(error)))?;

        let mut response = [0; 68];
        stream
            .read_exact(&mut response)
            .await
            .map_err(|error| self.error(PeerErrorInner::ReadError(error)))?;

        if response[0] != PROTOCOL_NAME.len() as u8
            || &response[1..20] != PROTOCOL_NAME
            || response[28..48] != self.info_hash
        {
            log::warn!("peer sent an invalid handshake");
            return Err(self.error(PeerErrorInner::BadPeer));
        }

        log::info!(
            "connected to peer: {}",
            stream.peer_addr().map_err(|error| {
                self.error(PeerErrorInner::Internal(format!(
                    "failed to read peer address: {error}"
                )))
            })?
        );
        Ok(())
    }

    async fn run_loop(&mut self, stream: &mut TcpStream) -> Result<(), PeerError> {
        let piece_count = METAINFO.info.pieces.len();
        let mut availability = vec![false; piece_count];
        let mut pending = None;
        let mut choked = true;

        self.send_message(stream, Message::Interested).await?;
        let result = self
            .message_loop(stream, &mut availability, &mut pending, &mut choked)
            .await;

        if let Some(piece) = pending {
            self.required_pieces.write().await.release(piece.index);
        }
        for (index, available) in availability.into_iter().enumerate() {
            if available {
                self.required_pieces.write().await.change_element(index, -1);
            }
        }

        result
    }

    async fn message_loop(
        &mut self,
        stream: &mut TcpStream,
        availability: &mut [bool],
        pending: &mut Option<PendingPiece>,
        choked: &mut bool,
    ) -> Result<(), PeerError> {
        loop {
            let Some((id, payload)) = self.read_message(stream).await? else {
                continue;
            };

            match id {
                0 => {
                    Self::expect_payload(&payload, 0, self.index)?;
                    *choked = true;
                }
                1 => {
                    Self::expect_payload(&payload, 0, self.index)?;
                    *choked = false;
                }
                2 | 3 => Self::expect_payload(&payload, 0, self.index)?,
                4 => {
                    Self::expect_payload(&payload, 4, self.index)?;
                    let index = u32::from_be_bytes(payload[..4].try_into().unwrap()) as usize;
                    if index >= availability.len() {
                        return Err(self.error(PeerErrorInner::BadPeer));
                    }
                    self.set_availability(availability, index, true).await;
                }
                5 => {
                    let expected_len = availability.len().div_ceil(8);
                    Self::expect_payload(&payload, expected_len, self.index)?;
                    for index in 0..availability.len() {
                        let has_piece = payload[index / 8] & (0x80 >> (index % 8)) != 0;
                        self.set_availability(availability, index, has_piece).await;
                    }
                }
                6 => Self::expect_payload(&payload, 12, self.index)?,
                7 => {
                    if payload.len() < 8 {
                        return Err(self.error(PeerErrorInner::BadPeer));
                    }
                    let index = u32::from_be_bytes(payload[..4].try_into().unwrap()) as usize;
                    let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap()) as usize;
                    self.handle_piece(
                        stream,
                        index,
                        begin,
                        &payload[8..],
                        availability,
                        pending,
                        *choked,
                    )
                    .await?;
                }
                8 => Self::expect_payload(&payload, 12, self.index)?,
                9 => Self::expect_payload(&payload, 2, self.index)?,
                _ => continue,
            }

            if !*choked {
                if pending.is_none() {
                    *pending = self.reserve_piece(availability).await?;
                }
                if let Some(piece) = pending.as_mut() {
                    self.request_more(stream, piece).await?;
                }
            }
        }
    }

    async fn handle_piece(
        &mut self,
        stream: &mut TcpStream,
        index: usize,
        begin: usize,
        block: &[u8],
        availability: &[bool],
        pending: &mut Option<PendingPiece>,
        choked: bool,
    ) -> Result<(), PeerError> {
        let Some(piece) = pending.as_mut() else {
            return Err(self.error(PeerErrorInner::BadPeer));
        };
        if index != piece.index {
            return Err(self.error(PeerErrorInner::BadPeer));
        }
        let Some(expected_length) = piece.outstanding.remove(&begin) else {
            return Err(self.error(PeerErrorInner::BadPeer));
        };
        if block.len() != expected_length {
            return Err(self.error(PeerErrorInner::BadPeer));
        }

        piece.data[begin..begin + block.len()].copy_from_slice(block);
        piece.received += block.len();
        if piece.received == piece.data.len() {
            let completed = pending.take().unwrap();
            if self
                .tx
                .send(PieceResponse {
                    index: completed.index,
                    buf: completed.data,
                })
                .is_err()
            {
                self.required_pieces.write().await.release(completed.index);
                return Err(self.error(PeerErrorInner::Internal(
                    "piece receiver has been closed".into(),
                )));
            }
            if !choked {
                *pending = self.reserve_piece(availability).await?;
                if let Some(piece) = pending.as_mut() {
                    self.request_more(stream, piece).await?;
                }
            }
        }
        Ok(())
    }

    async fn reserve_piece(
        &self,
        availability: &[bool],
    ) -> Result<Option<PendingPiece>, PeerError> {
        let piece_length = usize::try_from(METAINFO.info.piece_length)
            .ok()
            .filter(|length| *length > 0)
            .ok_or_else(|| {
                self.error(PeerErrorInner::Internal(
                    "torrent has an invalid piece length".into(),
                ))
            })?;
        let total_length = total_length(&METAINFO)
            .map_err(|error| self.error(PeerErrorInner::Internal(error.to_string())))?;
        let total_length = usize::try_from(total_length).map_err(|_| {
            self.error(PeerErrorInner::Internal(
                "torrent has an invalid total length".into(),
            ))
        })?;

        let Some(index) = self.required_pieces.write().await.reserve(availability) else {
            return Ok(None);
        };
        let Some(offset) = index.checked_mul(piece_length) else {
            self.required_pieces.write().await.release(index);
            return Err(self.error(PeerErrorInner::BadPeer));
        };
        let Some(remaining) = total_length.checked_sub(offset) else {
            self.required_pieces.write().await.release(index);
            return Err(self.error(PeerErrorInner::BadPeer));
        };
        let length = piece_length.min(remaining);

        if length == 0 {
            self.required_pieces.write().await.release(index);
            return Err(self.error(PeerErrorInner::Internal(
                "torrent piece has zero length".into(),
            )));
        }
        Ok(Some(PendingPiece::new(index, length)))
    }

    async fn request_more(
        &mut self,
        stream: &mut TcpStream,
        piece: &mut PendingPiece,
    ) -> Result<(), PeerError> {
        while piece.outstanding.len() < MAX_OUTSTANDING_REQUESTS
            && piece.next_begin < piece.data.len()
        {
            let begin = piece.next_begin;
            let length = BLOCK_LENGTH.min(piece.data.len() - begin);
            self.send_message(
                stream,
                Message::Request {
                    index: piece.index,
                    begin,
                    length,
                },
            )
            .await?;
            piece.outstanding.insert(begin, length);
            piece.next_begin += length;
        }
        Ok(())
    }

    async fn set_availability(&self, availability: &mut [bool], index: usize, has_piece: bool) {
        if availability[index] == has_piece {
            return;
        }
        availability[index] = has_piece;
        self.required_pieces
            .write()
            .await
            .change_element(index, if has_piece { 1 } else { -1 });
    }

    async fn read_message(
        &mut self,
        stream: &mut TcpStream,
    ) -> Result<Option<(u8, Vec<u8>)>, PeerError> {
        let mut length_bytes = [0; 4];
        stream
            .read_exact(&mut length_bytes)
            .await
            .map_err(|error| self.error(PeerErrorInner::ReadError(error)))?;
        let length = u32::from_be_bytes(length_bytes) as usize;
        if length == 0 {
            return Ok(None);
        }
        if length > MAX_MESSAGE_LENGTH {
            return Err(self.error(PeerErrorInner::BadPeer));
        }

        let mut message = vec![0; length];
        stream
            .read_exact(&mut message)
            .await
            .map_err(|error| self.error(PeerErrorInner::ReadError(error)))?;
        Ok(Some((message[0], message[1..].to_vec())))
    }

    fn expect_payload(payload: &[u8], length: usize, index: usize) -> Result<(), PeerError> {
        if payload.len() == length {
            Ok(())
        } else {
            Err(PeerError {
                index,
                inner: PeerErrorInner::BadPeer,
            })
        }
    }

    async fn send_message(
        &mut self,
        stream: &mut TcpStream,
        msg: Message,
    ) -> Result<(), PeerError> {
        let Some(bytes) = msg.to_bytes() else {
            return Err(self.error(PeerErrorInner::Internal(
                "peer message field exceeds the BitTorrent protocol limit".into(),
            )));
        };
        stream
            .write_all(&bytes)
            .await
            .map_err(|error| self.error(PeerErrorInner::WriteError(error)))
    }

    fn error(&self, inner: PeerErrorInner) -> PeerError {
        PeerError {
            index: self.index,
            inner,
        }
    }
}
