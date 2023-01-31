use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use hearth_network::auth::ServerAuthenticator;
use hearth_rpc::*;
use hearth_types::*;
use remoc::robs::hash_map::{HashMapSubscription, ObservableHashMap};
use remoc::rtc::{async_trait, LocalRwLock, ServerShared, ServerSharedMut};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info};

/// The constant peer ID for this peer (the server).
pub const SELF_PEER_ID: PeerId = PeerId(0);

/// The Hearth virtual space server program.
#[derive(Parser, Debug)]
pub struct Args {
    /// IP address and port to listen on.
    #[arg(short, long)]
    pub bind: SocketAddr,

    /// Password to use to authenticate with clients. Defaults to empty.
    #[arg(short, long, default_value = "")]
    pub password: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    hearth_core::init_logging();

    let authenticator = ServerAuthenticator::from_password(args.password.as_bytes()).unwrap();
    let authenticator = Arc::new(authenticator);

    info!("Binding to {:?}", args.bind);
    let listener = match TcpListener::bind(args.bind).await {
        Ok(l) => l,
        Err(err) => {
            error!("Failed to listen: {:?}", err);
            return;
        }
    };

    debug!("Creating peer API");
    let peer_info = PeerInfo { nickname: None };
    let peer_api = hearth_core::PeerApiImpl {
        info: peer_info.clone(),
    };

    let peer_api = Arc::new(peer_api);
    let (peer_api_server, peer_api) =
        PeerApiServerShared::<_, remoc::codec::Default>::new(peer_api, 1024);

    debug!("Spawning peer API server thread");
    tokio::spawn(async move {
        peer_api_server.serve(true).await;
    });

    debug!("Creating peer provider");
    let mut peer_provider = PeerProviderImpl::new();
    peer_provider.add_peer(SELF_PEER_ID, peer_api, peer_info);
    let peer_provider = Arc::new(LocalRwLock::new(peer_provider));

    info!("Listening");
    loop {
        let (socket, addr) = match listener.accept().await {
            Ok(v) => v,
            Err(err) => {
                error!("Listening error: {:?}", err);
                continue;
            }
        };

        info!("Connection from {:?}", addr);
        let peer_provider = peer_provider.clone();
        let authenticator = authenticator.clone();
        tokio::task::spawn(async move {
            on_accept(peer_provider, authenticator, socket, addr).await;
        });
    }
}

async fn on_accept(
    peer_provider: Arc<LocalRwLock<PeerProviderImpl>>,
    authenticator: Arc<ServerAuthenticator>,
    mut client: TcpStream,
    addr: SocketAddr,
) {
    info!("Authenticating with client {:?}", addr);
    let session_key = match authenticator.login(&mut client).await {
        Ok(key) => key,
        Err(err) => {
            error!("Authentication error: {:?}", err);
            return;
        }
    };

    info!("Successfully authenticated");
    use hearth_network::encryption::{AsyncDecryptor, AsyncEncryptor, Key};
    let client_key = Key::from_client_session(&session_key);
    let server_key = Key::from_server_session(&session_key);

    let (client_rx, client_tx) = tokio::io::split(client);
    let client_rx = AsyncDecryptor::new(&client_key, client_rx);
    let client_tx = AsyncEncryptor::new(&server_key, client_tx);

    debug!("Initializing Remoc connection");
    use remoc::rch::base::{Receiver, Sender};
    let cfg = remoc::Cfg::default();
    let (conn, mut tx, mut rx): (_, Sender<ServerOffer>, Receiver<ClientOffer>) =
        match remoc::Connect::io(cfg, client_rx, client_tx).await {
            Ok(v) => v,
            Err(err) => {
                error!("Remoc connection failure: {:?}", err);
                return;
            }
        };

    debug!("Spawning Remoc connection thread");
    let join_connection = tokio::spawn(conn);

    let (peer_provider_server, peer_provider_client) =
        PeerProviderServerSharedMut::<_, remoc::codec::Default>::new(peer_provider.clone(), 1024);

    debug!("Spawning peer provider server");
    tokio::spawn(async move {
        debug!("Running peer provider server");
        peer_provider_server.serve(true).await;
    });

    debug!("Generating peer ID");
    let peer_id = peer_provider.write().await.get_next_peer();
    info!("Generated peer ID for new client: {:?}", peer_id);

    debug!("Sending server offer to client");
    tx.send(ServerOffer {
        peer_provider: peer_provider_client,
        new_id: peer_id,
    })
    .await
    .unwrap();

    debug!("Receiving client offer");
    let offer: ClientOffer = match rx.recv().await {
        Ok(Some(o)) => o,
        Ok(None) => {
            error!("Client hung up while waiting for offer");
            return;
        }
        Err(err) => {
            error!("Failed to receive client offer: {:?}", err);
            return;
        }
    };

    debug!("Getting peer {:?} info", peer_id);
    let peer_info = match offer.peer_api.get_info().await {
        Ok(i) => i,
        Err(err) => {
            error!("Failed to retrieve client peer info: {:?}", err);
            return;
        }
    };

    debug!("Adding peer {:?} to peer provider", peer_id);
    peer_provider
        .write()
        .await
        .add_peer(peer_id, offer.peer_api, peer_info);

    debug!("Waiting to join Remoc connection thread");
    match join_connection.await {
        Err(err) => {
            error!(
                "Tokio error while joining peer {:?} connection thread: {:?}",
                peer_id, err
            );
        }
        Ok(Err(remoc::chmux::ChMuxError::StreamClosed)) => {
            info!("Peer {:?} disconnected", peer_id);
        }
        Ok(Err(err)) => {
            error!(
                "Remoc chmux error while joining peer {:?} connection thread: {:?}",
                peer_id, err
            );
        }
        Ok(Ok(())) => {}
    }

    debug!("Removing peer from peer provider");
    peer_provider.write().await.remove_peer(peer_id);
}

struct PeerProviderImpl {
    next_peer: PeerId,
    peer_list: ObservableHashMap<PeerId, PeerInfo>,
    peer_apis: HashMap<PeerId, PeerApiClient>,
}

#[async_trait]
impl PeerProvider for PeerProviderImpl {
    async fn find_peer(&self, id: PeerId) -> CallResult<Option<PeerApiClient>> {
        Ok(self.peer_apis.get(&id).cloned())
    }

    async fn follow_peer_list(&self) -> CallResult<HashMapSubscription<PeerId, PeerInfo>> {
        Ok(self.peer_list.subscribe(1024))
    }
}

impl PeerProviderImpl {
    pub fn new() -> Self {
        Self {
            next_peer: PeerId(1), // start from 1 to accomodate [SELF_PEER_ID]
            peer_list: Default::default(),
            peer_apis: Default::default(),
        }
    }

    pub fn get_next_peer(&mut self) -> PeerId {
        let id = self.next_peer;
        self.next_peer.0 += 1;
        id
    }

    pub fn add_peer(&mut self, id: PeerId, api: PeerApiClient, info: PeerInfo) {
        self.peer_list.insert(id, info);
        self.peer_apis.insert(id, api);
    }

    pub fn remove_peer(&mut self, id: PeerId) {
        self.peer_list.remove(&id);
        self.peer_apis.remove(&id);
    }
}
