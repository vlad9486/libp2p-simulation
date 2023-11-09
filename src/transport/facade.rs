use std::{
    io,
    pin::Pin,
    task::{Context, Poll, self},
    collections::VecDeque,
    sync::{Arc, Mutex},
    net::{SocketAddr, IpAddr},
};

use futures::future::{Ready, self};
use libp2p_core::{
    Transport,
    transport::{ListenerId, TransportError, TransportEvent},
    Multiaddr,
    multiaddr::Protocol,
};

use super::{
    stream::Stream,
    state::Registry,
    back::{Back, ActorIdCell},
};

pub struct Front {
    id: ActorIdCell,
    registry: Arc<Mutex<Registry>>,
    events: VecDeque<Event<Self>>,
}

impl Front {
    pub(crate) fn new(registry: Arc<Mutex<Registry>>) -> (Back, Front) {
        let id = ActorIdCell::default();
        (
            Back::new(registry.clone(), id.clone()),
            Front {
                id,
                registry,
                events: VecDeque::new(),
            },
        )
    }
}

type Event<T> = TransportEvent<<T as Transport>::ListenerUpgrade, <T as Transport>::Error>;

impl Transport for Front {
    type Output = Stream;

    type Error = io::Error;

    // TODO: should be a future, pending until actor recv the dial message
    type Dial = Ready<Result<Self::Output, Self::Error>>;

    type ListenerUpgrade = Ready<Result<Self::Output, Self::Error>>;

    fn listen_on(
        &mut self,
        listener_id: ListenerId,
        addr: Multiaddr,
    ) -> Result<(), TransportError<Self::Error>> {
        let actor_id = self.id.get().expect("must be initialized");

        let mut lock = self.registry.lock().expect("poisoned");
        let socket_addr = multiaddr_to_socket_addr(addr.clone())
            .ok_or(TransportError::MultiaddrNotSupported(addr))?;
        let addr = lock
            .allocate_addr(actor_id, socket_addr, listener_id)
            .ok_or(io::ErrorKind::AddrNotAvailable.into())
            .map_err(TransportError::Other)?;
        drop(lock);

        self.events.push_back(TransportEvent::NewAddress {
            listener_id,
            listen_addr: socket_addr_to_multiaddr(addr),
        });
        Ok(())
    }

    fn remove_listener(&mut self, id: ListenerId) -> bool {
        let actor_id = self.id.get().expect("must be initialized");

        if self
            .registry
            .lock()
            .expect("poisoned")
            .dealloc_addr(actor_id, id)
        {
            self.events.push_back(TransportEvent::ListenerClosed {
                listener_id: id,
                reason: Ok(()),
            });
            true
        } else {
            false
        }
    }

    fn dial(&mut self, addr: Multiaddr) -> Result<Self::Dial, TransportError<Self::Error>> {
        let actor_id = self.id.get().expect("must be initialized");
        let (local, remote) = Stream::pair();

        let socket_addr = multiaddr_to_socket_addr(addr.clone())
            .ok_or(TransportError::MultiaddrNotSupported(addr))?;

        self.registry
            .lock()
            .expect("poisoned")
            .register_stream(remote, actor_id, socket_addr)
            .map_err(TransportError::Other)?;

        Ok(future::ready(Ok(local)))
    }

    fn dial_as_listener(
        &mut self,
        addr: Multiaddr,
    ) -> Result<Self::Dial, TransportError<Self::Error>> {
        self.dial(addr)
    }

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Event<Self>> {
        match self.events.pop_front() {
            Some(e) => Poll::Ready(e),
            None => {
                let mut lock = self.registry.lock().expect("poisoned");
                let actor_id = self.id.get().expect("must be initialized");
                let (listener_id, local_addr, addr, stream) =
                    task::ready!(lock.poll_stream(actor_id, cx));
                Poll::Ready(TransportEvent::Incoming {
                    listener_id,
                    upgrade: future::ready(Ok(stream)),
                    local_addr: socket_addr_to_multiaddr(local_addr),
                    send_back_addr: socket_addr_to_multiaddr(addr),
                })
            }
        }
    }

    fn address_translation(&self, listen: &Multiaddr, observed: &Multiaddr) -> Option<Multiaddr> {
        let _ = listen;
        Some(observed.clone())
    }
}

fn multiaddr_to_socket_addr(v: Multiaddr) -> Option<SocketAddr> {
    let ip = v.iter().find_map(|p| match p {
        Protocol::Ip4(ip) => Some(ip.into()),
        Protocol::Ip6(ip) => Some(ip.into()),
        _ => None,
    })?;
    let port = v.iter().find_map(|p| match p {
        Protocol::Tcp(v) => Some(v),
        _ => None,
    })?;
    Some(SocketAddr::new(ip, port))
}

fn socket_addr_to_multiaddr(v: SocketAddr) -> Multiaddr {
    let mut a = Multiaddr::empty();
    match v.ip() {
        IpAddr::V4(ip) => a.push(Protocol::Ip4(ip)),
        IpAddr::V6(ip) => a.push(Protocol::Ip6(ip)),
    }
    a.push(Protocol::Tcp(v.port()));
    a
}
