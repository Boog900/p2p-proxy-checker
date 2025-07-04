use dashmap::DashSet;
use futures::{future::BoxFuture, stream, FutureExt};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::{
    collections::HashSet,
    convert::Infallible,
    fs::OpenOptions,
    io::Write,
    net::{IpAddr, SocketAddr},
    sync::{LazyLock, OnceLock},
    task::Poll,
    time::Duration,
};
use tokio::{
    sync::{mpsc, Semaphore},
    time::{sleep, timeout},
};
use tower::{make::Shared, util::MapErr, Service, ServiceExt};
use tracing::error;
use tracing_subscriber::{filter::LevelFilter, FmtSubscriber};

use cuprate_p2p_core::{
    client::{
        handshaker::builder::{DummyCoreSyncSvc, DummyProtocolRequestHandler},
        ConnectRequest, Connector, HandshakerBuilder, InternalPeerID,
    },
    services::{AddressBookRequest, AddressBookResponse},
    BroadcastMessage, ClearNet, NetZoneAddress, Network, NetworkZone, PeerRequest, PeerResponse,
};
use cuprate_wire::{
    common::PeerSupportFlags, AdminRequestMessage, AdminResponseMessage, BasicNodeData,
};

static SCANNED_NODES: LazyLock<DashSet<SocketAddr>> = LazyLock::new(|| DashSet::new());

static CONNECTOR: OnceLock<
    Connector<
        ClearNet,
        AddressBookService,
        DummyCoreSyncSvc,
        MapErr<Shared<DummyProtocolRequestHandler>, fn(Infallible) -> tower::BoxError>,
        fn(InternalPeerID<<ClearNet as NetworkZone>::Addr>) -> stream::Pending<BroadcastMessage>,
    >,
> = OnceLock::new();

static BAD_PEERS_CHANNEL: OnceLock<mpsc::Sender<(SocketAddr, Vec<u64>, bool)>> = OnceLock::new();

static CONNECTION_SEMAPHORE: Semaphore = Semaphore::const_new(100);

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    FmtSubscriber::builder()
        .with_max_level(LevelFilter::DEBUG)
        .init();

    let handshaker = HandshakerBuilder::<ClearNet>::new(BasicNodeData {
        my_port: 0,
        network_id: Network::Mainnet.network_id(),
        peer_id: rand::random(),
        support_flags: PeerSupportFlags::FLUFFY_BLOCKS,
        rpc_port: 0,
        rpc_credits_per_hash: 0,
    })
    .with_address_book(AddressBookService)
    .build();

    let connector = Connector::new(handshaker);

    let _ = CONNECTOR.get_or_init(|| connector.clone());

    let (bad_peers_tx, mut bad_peers_rx) = mpsc::channel(508);

    BAD_PEERS_CHANNEL.set(bad_peers_tx).unwrap();

    // seed nodes
    [
        "176.9.0.187:18080",
        "88.198.163.90:18080",
        "66.85.74.134:18080",
        "51.79.173.165:18080",
        "192.99.8.110:18080",
        "37.187.74.171:18080",
        "77.172.183.193:18080",
    ]
    .into_iter()
    .for_each(|ip| {
        tokio::spawn(check_node(ip.parse().unwrap()));
    });

    let mut bad_peers = HashSet::new();
    let mut bad_peers_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("bad_peers.txt")
        .unwrap();

    let mut good_peers_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("good_peers.txt")
        .unwrap();

    loop {
        let (peer, peer_ids, peer_bad) = bad_peers_rx.recv().await.unwrap();

        if peer_bad {
            error!("Found bad peer: {peer:?}");
            if !bad_peers.insert(peer) {
                continue;
            }

            bad_peers_file
                .write_fmt(format_args!("peer: {peer:?}, peer_ids: {peer_ids:?}, \n"))
                .unwrap();
        } else {
            good_peers_file
                .write_fmt(format_args!("peer: {peer:?}, peer_ids: {peer_ids:?}, \n"))
                .unwrap();
        }
    }
}

async fn check_node(addr: SocketAddr) -> Result<(), tower::BoxError> {
    let _guard = CONNECTION_SEMAPHORE.acquire().await.unwrap();

    let mut connector = CONNECTOR.get().unwrap().clone();

    let mut client = timeout(
        Duration::from_secs(5),
        connector
            .ready()
            .await?
            .call(ConnectRequest { addr, permit: None }),
    )
    .await??;

    let PeerResponse::Admin(AdminResponseMessage::Ping(ping)) = client
        .ready()
        .await?
        .call(PeerRequest::Admin(AdminRequestMessage::Ping))
        .await?
    else {
        unreachable!();
    };

    let PeerResponse::Admin(AdminResponseMessage::Ping(ping_2)) = client
        .ready()
        .await?
        .call(PeerRequest::Admin(AdminRequestMessage::Ping))
        .await?
    else {
        unreachable!();
    };

    let PeerResponse::Admin(AdminResponseMessage::Ping(ping_3)) = client
        .ready()
        .await?
        .call(PeerRequest::Admin(AdminRequestMessage::Ping))
        .await?
    else {
        unreachable!();
    };

    let peer_ids = vec![
        client.info.basic_node_data.peer_id,
        ping.peer_id,
        ping_2.peer_id,
        ping_3.peer_id,
    ];
    let bad = client.info.basic_node_data.peer_id != ping.peer_id
        || ping.peer_id != ping_2.peer_id
        || ping_2.peer_id != ping_3.peer_id;

    BAD_PEERS_CHANNEL
        .get()
        .unwrap()
        .send((addr, peer_ids, bad))
        .await?;

    Ok(())
}

#[derive(Clone)]
pub struct AddressBookService;

impl Service<AddressBookRequest<ClearNet>> for AddressBookService {
    type Error = tower::BoxError;
    type Response = AddressBookResponse<ClearNet>;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: AddressBookRequest<ClearNet>) -> Self::Future {
        async {
            match req {
                AddressBookRequest::IncomingPeerList(peers) => {
                    for mut peer in peers {
                        peer.adr.make_canonical();
                        if SCANNED_NODES.insert(peer.adr) {
                            tokio::spawn(async move {
                                if check_node(peer.adr).await.is_err() {
                                    SCANNED_NODES.remove(&peer.adr);
                                }
                            });
                        }
                    }

                    Ok(AddressBookResponse::Ok)
                }
                AddressBookRequest::NewConnection { .. } => Ok(AddressBookResponse::Ok),
                AddressBookRequest::TakeRandomWhitePeer { .. } => Err("no peers".into()),
                AddressBookRequest::TakeRandomGrayPeer { .. } => Err("no peers".into()),
                AddressBookRequest::TakeRandomPeer { .. } => Err("no peers".into()),
                AddressBookRequest::GetWhitePeers(_) => Ok(AddressBookResponse::Peers(vec![])),
                AddressBookRequest::PeerlistSize => Err("no peers".into()),
                AddressBookRequest::ConnectionCount => Err("no peers".into()),
                AddressBookRequest::SetBan(_) => Err("no peers".into()),
                AddressBookRequest::GetBan(_) => Err("no peers".into()),
                AddressBookRequest::GetBans => Err("no peers".into()),
                AddressBookRequest::ConnectionInfo => Err("no peers".into()),
            }
        }
        .boxed()
    }
}
