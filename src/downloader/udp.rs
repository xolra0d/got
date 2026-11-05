use crate::{
    downloader::{METAINFO, info_hash, total_length},
    error::Error,
};
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    random::{self},
    sync::{
        Arc,
        atomic::{AtomicI32, AtomicI64, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::UdpSocket,
    select,
    sync::{Notify, mpsc::Sender},
    time::{self, Instant},
};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, big_endian};

#[derive(Debug, Clone)]
#[repr(i32)]
enum Action {
    Connect = 0,
    Announce = 1,
    Error = 3,
}

#[derive(Debug, Clone)]
#[repr(i32)]
enum Event {
    r#None = 0,
    Completed = 1,
    Started = 2,
    Stopped = 3,
}

impl Event {
    fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Completed,
            2 => Self::Started,
            3 => Self::Stopped,
            _ => panic!("invalid event code: {value}"),
        }
    }
}

const POSSIBLE_BIND_PORTS: [u16; 8] = [6887, 6681, 6882, 6883, 6884, 6885, 6886, 6888];
const PROTOCOL_ID: i64 = 0x41727101980;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RETRIES: usize = 8;

#[derive(IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct ConnectRequest {
    protocol_id: big_endian::I64,
    action: big_endian::I32,
    transaction_id: big_endian::I32,
}

#[derive(FromBytes, KnownLayout, Immutable)]
#[repr(C)]
struct ConnectResponseHeader {
    action: big_endian::I32,
    transaction_id: big_endian::I32,
    // intentionally missing connection_id | error message
}

pub async fn connect(socket: &UdpSocket) -> Result<i64, Error> {
    let transaction_id: i32 = random::random(..);
    let request = ConnectRequest {
        protocol_id: PROTOCOL_ID.into(),
        action: (Action::Connect as i32).into(),
        transaction_id: transaction_id.into(),
    };

    for _ in 1..=MAX_RETRIES {
        let res = socket.send(&request.as_bytes()).await;
        match res {
            Ok(n) if n != size_of::<ConnectRequest>() => {
                log::warn!(
                    "sent unexpected number of bytes during connect request (expected: {}, sent: {n}), retrying...",
                    size_of::<ConnectRequest>()
                );
            }
            Ok(_) => break,
            Err(error) => {
                log::warn!("received error during during connect request: {error}, restarting...");
            }
        }
        time::sleep(Duration::from_secs(1)).await;
    }

    let mut response = [0u8; 256];

    for i in 1..=MAX_RETRIES {
        let received = match time::timeout(
            REQUEST_TIMEOUT * (2u32.pow(i as u32)),
            socket.recv(&mut response),
        )
        .await
        .ok() // conv timeout to Option::None
        {
            None => {
                log::warn!("received timeout during attempt {i}, connect action, retrying...");
                continue;
            }
            Some(Ok(value)) => value,
            Some(Err(error)) => {
                log::warn!("received error during connect action: {error}, retrying...");
                continue;
            }
        };

        if received < 16 {
            log::warn!(
                "received too few bytes (expected at least 16, received {received} instead), retrying..."
            );
            continue;
        }
        let frame = &response[..received];
        let header = match ConnectResponseHeader::ref_from_prefix(frame) {
            Ok((h, _)) => h,
            Err(error) => {
                log::error!(
                    "failed to parse connect response! It should not happen, error:{error}"
                );
                continue;
            }
        };

        if header.transaction_id != transaction_id {
            log::warn!(
                "received transaction id does not match sent one (expected: {transaction_id}, received; {}), retrying...",
                header.transaction_id
            );
            continue;
        }

        if header.action == Action::Error as i32 {
            let message = String::from_utf8_lossy(&frame[8..]);
            log::warn!("received error during connect action: {message}");
            continue;
        }
        if header.action != Action::Connect as i32 {
            log::warn!(
                "received unknown action code ({}) during connect action",
                header.action
            );
            continue;
        }

        let connection_id = i64::from_be_bytes(
            frame[8..]
                .try_into()
                .expect("length of received is at least size of header + connection id = 16"),
        );

        return Ok(connection_id);
    }

    log::error!("max retries reached during connect attempt, exiting...");
    Err(Error::Connect)
}

#[derive(IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct AnnounceRequest {
    connection_id: big_endian::I64,
    action: big_endian::I32,
    transaction_id: big_endian::I32,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    downloaded: big_endian::I64,
    left: big_endian::I64,
    uploaded: big_endian::I64,
    event: big_endian::I32,
    ip_address: big_endian::I32,
    key: big_endian::I32,
    num_want: big_endian::I32,
    port: big_endian::I16,
}

#[derive(FromBytes, KnownLayout, Immutable)]
#[repr(C)]
struct AnnounceResponseHeader {
    action: big_endian::I32,
    transaction_id: big_endian::I32,
    // intentionally missing fields | error message
}

#[derive(FromBytes, KnownLayout, Immutable)]
#[repr(C)]
struct AnnounceResponseBody {
    interval: big_endian::I32,
    leechers: big_endian::I32,
    seeders: big_endian::I32,
    // peers field is intentionally missing
}

#[derive(Debug, Clone)]
pub struct AnnounceResponse {
    pub interval: i32,
    pub leechers: i32,
    pub seeders: i32,
    pub peers: Vec<SocketAddr>,
}

async fn announce(
    socket: &UdpSocket,
    connection_id: i64,
    peer_id: [u8; 20],
    downloaded: i64,
    uploaded: i64,
    event: Event,
) -> Result<AnnounceResponse, Error> {
    let transaction_id: i32 = random::random(..);

    let request = AnnounceRequest {
        connection_id: connection_id.into(),
        action: (Action::Announce as i32).into(),
        transaction_id: transaction_id.into(),
        info_hash: info_hash(&METAINFO).expect("TODO"),
        peer_id,
        downloaded: downloaded.into(),
        left: (total_length(&METAINFO).expect("TODO") - downloaded).into(),
        uploaded: uploaded.into(),
        event: (event as i32).into(), // TODO
        ip_address: 0.into(),
        key: 0.into(), // TODO: An additional identification that is not shared with any other peers. It is intended to allow a client to prove their identity should their IP address change.
        num_want: (-1i32).into(),
        port:(socket.local_addr().map_err(|error| Error::Internal(format!("failed to get address from socket. If this happened, either another process (e.g., antivirus) closed fd, OR internal system exhaustion: {error}")))?.port() as i16).into(),
    };

    for _ in 1..=MAX_RETRIES {
        let res = socket.send(request.as_bytes()).await;
        match res {
            Ok(n) if n != size_of::<AnnounceRequest>() => {
                log::warn!(
                    "sent unexpected number of bytes during announce request (expected: {}, sent: {n}), retrying...",
                    size_of::<AnnounceRequest>()
                );
            }
            Ok(_) => break,
            Err(error) => {
                log::warn!("received error during during announce request: {error}, restarting...");
            }
        }
        time::sleep(Duration::from_secs(1)).await;
    }

    let mut response = [0u8; 256];

    for i in 1..=MAX_RETRIES {
        let received = match time::timeout(
            REQUEST_TIMEOUT * (2u32.pow(i as u32)),
            socket.recv(&mut response),
        )
        .await
        .ok() // conv timeout to Option::None
        {
            None => {
                log::warn!("received timeout during attempt {i}, announce action, retrying...");
                continue;
            }
            Some(Ok(value)) => value,
            Some(Err(error)) => {
                log::warn!("received error during announce action: {error}, retrying...");
                continue;
            }
        };

        if received < 20 {
            log::warn!(
                "received too few bytes (expected at least 20, received {received} instead), retrying..."
            );
            continue;
        }

        let frame = &response[..received];
        let header = match AnnounceResponseHeader::ref_from_prefix(frame) {
            Ok((h, _)) => h,
            Err(error) => {
                log::error!(
                    "failed to parse announce response! It should not happen, error:{error}"
                );
                continue;
            }
        };

        if header.transaction_id != transaction_id {
            log::warn!(
                "received transaction id does not match sent one (expected: {transaction_id}, received; {}), retrying...",
                header.transaction_id
            );
            continue;
        }

        if header.action == Action::Error as i32 {
            let message = String::from_utf8_lossy(&frame[8..]);
            log::warn!("received error during announce action: {message}");
            continue;
        }
        if header.action != Action::Announce as i32 {
            log::warn!(
                "received unknown action code ({}) during announce action",
                header.action
            );
            continue;
        }

        let (body, _) =
            AnnounceResponseBody::ref_from_prefix(&frame[size_of::<AnnounceResponseHeader>()..])
                .expect("TODO");

        let peers = frame
            [(size_of::<AnnounceResponseHeader>() + size_of::<AnnounceResponseBody>())..]
            .as_chunks::<6>()
            .0
            .iter()
            .map(|peer| {
                SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::new(peer[0], peer[1], peer[2], peer[3]),
                    u16::from_be_bytes([peer[4], peer[5]]),
                ))
            })
            .collect();

        let res = AnnounceResponse {
            interval: body.interval.get(),
            leechers: body.leechers.get(),
            seeders: body.seeders.get(),
            peers,
        };
        return Ok(res);
    }

    log::error!("max retries reached during announce attempt, exiting...");
    Err(Error::Announce)
}

pub async fn start_announce(
    peer_id: [u8; 20],
    tx: Sender<SocketAddr>,
    downloaded: AtomicI64,
    uploaded: AtomicI64,
    event: AtomicI32,
    force_announce: Arc<Notify>,
) -> Result<(), Error> {
    let addrs: Vec<SocketAddr> = POSSIBLE_BIND_PORTS
        .iter()
        .map(|port| SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, *port)))
        .collect();
    let socket = UdpSocket::bind(addrs.as_slice())
        .await
        .map_err(Error::SocketBind)?;
    socket
        .connect(&METAINFO.announce.socket())
        .await
        .map_err(|e| Error::SocketConnect(e.to_string()))?;

    let mut connection_id = connect(&socket).await?;
    let mut connection_id_interval = Instant::now();

    let ann = announce(
        &socket,
        connection_id,
        peer_id,
        downloaded.load(Ordering::SeqCst),
        uploaded.load(Ordering::SeqCst),
        Event::from_i32(event.load(Ordering::SeqCst)),
    )
    .await?;

    log::debug!("received announce response: {ann:#?}");

    let can_send = tx.capacity();
    log::debug!("can send to channel: {can_send}");
    for addr in &ann.peers[..can_send.min(ann.peers.len())] {
        if tx.send(*addr).await.is_err() {
            log::debug!("downloader closed channel, ending tracker comms");
            return Ok(());
        }
        log::debug!("sent an address: {addr}");
    }

    let mut announce_interval = time::interval(Duration::from_secs(ann.interval as u64));
    announce_interval.tick().await;

    loop {
        select! {
            _ = announce_interval.tick() => {}
            _ = force_announce.notified() => {
                announce_interval.reset();
            }
        }

        if connection_id_interval.elapsed() > Duration::from_mins(1) {
            connection_id = connect(&socket).await?;
            connection_id_interval = Instant::now();
        }
        let ann_res = announce(&socket, connection_id, peer_id, 0, 0, Event::None).await?;
        let can_send = tx.capacity();
        for addr in &ann_res.peers[..can_send.min(ann_res.peers.len())] {
            if tx.send(*addr).await.is_err() {
                log::debug!("downloader closed channel, ending tracker comms");
                return Ok(());
            }
            log::debug!("sent an address: {addr}");
        }
    }
}
