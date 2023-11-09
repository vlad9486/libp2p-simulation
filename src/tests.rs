use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use libp2p_identity::PeerId;

use libp2p_core::{self as core, Multiaddr, Transport};
use libp2p_identity as identity;
use libp2p_kad as kad;
use libp2p_noise as noise;
use libp2p_pnet as pnet;
use libp2p_swarm::{self as swarm, NetworkBehaviour, SwarmEvent, THandlerErr};
use libp2p_yamux as yamux;

use crate::{Application, Front, Registry, SwarmActor};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct KadPeer {
    config: Config,
    peers: BTreeMap<PeerId, ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Config {
    peer_id: PeerId,
    seed_node_peer_id: PeerId,
    seed_node_addr: Multiaddr,
}

impl Config {
    pub fn new(peer_id: PeerId, seed_node_peer_id: PeerId, seed_node_addr: Multiaddr) -> Self {
        Config {
            peer_id,
            seed_node_peer_id,
            seed_node_addr,
        }
    }
}

impl SwarmActor for KadPeer {
    type Behaviour = kad::Behaviour<kad::store::MemoryStore>;

    type Config = Config;

    fn init(config: &Self::Config, swarm: &mut swarm::Swarm<Self::Behaviour>) -> Self {
        let mut addr = config.seed_node_addr.clone();
        addr.push(core::multiaddr::Protocol::P2p(
            config.seed_node_peer_id.clone(),
        ));
        swarm.behaviour_mut().set_mode(Some(kad::Mode::Server));
        if config.peer_id == config.seed_node_peer_id {
            swarm.listen_on(addr).unwrap();
        } else {
            let this = "/ip4/127.0.0.1/tcp/0".parse::<Multiaddr>().unwrap();
            swarm.listen_on(this).unwrap();
            swarm
                .behaviour_mut()
                .add_address(&config.seed_node_peer_id, config.seed_node_addr.clone());
            swarm.behaviour_mut().bootstrap().unwrap();
        }
        KadPeer {
            config: config.clone(),
            peers: BTreeMap::default(),
        }
    }

    fn on_event(
        &mut self,
        swarm: &mut swarm::Swarm<Self::Behaviour>,
        event: SwarmEvent<
            <Self::Behaviour as NetworkBehaviour>::ToSwarm,
            THandlerErr<Self::Behaviour>,
        >,
    ) {
        log::info!("{}: {event:?}", self.config.peer_id);
        match event {
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                self.peers.insert(peer_id, ());
            }
            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                self.peers.remove(&peer_id);
            }
            _ => (),
        }
        let _ = swarm;
    }
}

fn create_actor(
    chain_id: &str,
    local_key: identity::Keypair,
    registry: Arc<Mutex<Registry>>,
    config: Config,
) -> Application<KadPeer> {
    let local_peer_id = PeerId::from(local_key.public());

    let transport_factory = |front: Front| {
        let pnet = {
            use blake2::{
                digest::{generic_array::GenericArray, Update, VariableOutput},
                Blake2bVar,
            };

            let mut key = GenericArray::default();
            Blake2bVar::new(32)
                .expect("valid constant")
                .chain(b"/coda/0.0.1/")
                .chain(chain_id)
                .finalize_variable(&mut key)
                .expect("good buffer size");

            pnet::PnetConfig::new(pnet::PreSharedKey::new(key.into()))
        };

        let noise_config = noise::Config::new(&local_key).unwrap();
        let mut yamux_config = yamux::Config::default();
        yamux_config.set_protocol_name("/coda/yamux/1.0.0");
        front
            .and_then(move |socket, _| pnet.handshake(socket))
            .upgrade(core::upgrade::Version::V1)
            .authenticate(noise_config)
            .multiplex(yamux_config)
            .timeout(Duration::from_secs(60))
            .boxed()
    };

    let kad_config = {
        let mut c = kad::Config::default();
        c.set_protocol_names(vec![swarm::StreamProtocol::new("/coda/kad/1.0.0")]);
        c
    };
    let kademlia = kad::Behaviour::with_config(
        local_peer_id,
        kad::store::MemoryStore::new(local_peer_id),
        kad_config,
    );
    Application::new(registry, local_peer_id, config, transport_factory, kademlia)
}

fn create_system() -> impl IntoIterator<Item = Application<KadPeer>> {
    use std::iter;

    let chain_id = "3c41383994b87449625df91769dff7b507825c064287d30fada9286f3f1cb15e";
    let registry = Arc::new(Mutex::new(Registry::default()));
    let seed_node_keypair = identity::Keypair::generate_ed25519();
    let seed_node_peer_id = seed_node_keypair.public().to_peer_id();
    let seed_node_addr = "/ip4/127.0.0.1/tcp/8302".parse::<Multiaddr>().unwrap();

    iter::once(create_actor(
        chain_id,
        seed_node_keypair,
        registry.clone(),
        Config::new(
            seed_node_peer_id.clone(),
            seed_node_peer_id.clone(),
            seed_node_addr.clone(),
        ),
    ))
    .chain(
        iter::repeat_with(move || {
            let keypair = identity::Keypair::generate_ed25519();
            let peer_id = keypair.public().to_peer_id();
            create_actor(
                chain_id,
                keypair,
                registry.clone(),
                Config::new(peer_id, seed_node_peer_id.clone(), seed_node_addr.clone()),
            )
        })
        .take(1),
    )
}

#[test]
fn simulate() {
    env_logger::try_init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    )
    .unwrap_or_default();

    Application::run(create_system());
}

#[cfg(feature = "stateright")]
#[test]
fn check() {
    use stateright::{
        actor::{ActorModel, LossyNetwork, Network},
        Checker, Expectation, Model,
    };

    env_logger::try_init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    )
    .unwrap_or_default();

    let checker = ActorModel::new((), ())
        .init_network(Network::Ordered(BTreeMap::new()))
        .lossy_network(LossyNetwork::No)
        .actors(create_system())
        .property(Expectation::Eventually, "find peer", |_model, state| {
            state
                .actor_states
                .iter()
                .all(|state| !state.peers.is_empty())
        })
        .checker()
        // .serve(SocketAddr::from(([127, 0, 0, 1], 8080)));
        .spawn_bfs()
        .join();
    checker.assert_properties();
}
