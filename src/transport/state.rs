use std::{
    net::SocketAddr,
    collections::{HashMap, BTreeMap},
    io,
    sync::mpsc,
    task::{Poll, Context, Waker},
};

use libp2p_core::transport::ListenerId;

use super::{stream::Stream, allocator::Allocator, message::Msg, back::ActorId};

#[derive(Default, Debug)]
pub struct Registry {
    allocator: Allocator,
    actor_by_addr: HashMap<SocketAddr, ActorId>,

    actor_states: HashMap<ActorId, ActorState>,
}

#[derive(Default, Debug)]
struct ActorState {
    waker: Option<Waker>,
    by_listener: HashMap<ListenerId, SocketAddr>,
    bindings: BTreeMap<SocketAddr, Binding>,
}

#[derive(Debug)]
struct Binding {
    listener: ListenerId,
    pending_outgoing: BTreeMap<SocketAddr, Stream>,
    pending_incoming: BTreeMap<SocketAddr, ()>,
    streams: HashMap<SocketAddr, Stream>,
}

impl Registry {
    pub fn cleanup(&mut self, id: ActorId) {
        let Some(state) = self.actor_states.remove(&id) else {
            return;
        };

        for (addr, binding) in state.bindings {
            self.allocator.dealloc_addr(&addr);
            self.actor_by_addr.remove(&addr);
            // TODO: what with current streams
            let _ = binding;
        }
    }

    pub fn allocate_addr(
        &mut self,
        actor_id: ActorId,
        addr: SocketAddr,
        listener_id: ListenerId,
    ) -> Option<SocketAddr> {
        let addr = self.allocator.allocate_addr(addr)?;
        self.actor_by_addr.insert(addr.clone(), actor_id);
        let state = self.actor_states.entry(actor_id).or_default();
        state.by_listener.insert(listener_id, addr.clone());
        state.bindings.insert(
            addr.clone(),
            Binding {
                listener: listener_id,
                pending_outgoing: BTreeMap::default(),
                pending_incoming: BTreeMap::default(),
                streams: HashMap::default(),
            },
        );
        Some(addr)
    }

    pub fn dealloc_addr(&mut self, actor_id: ActorId, listener_id: ListenerId) -> bool {
        if let Some(state) = self.actor_states.get_mut(&actor_id) {
            if let Some(addr) = state.by_listener.remove(&listener_id) {
                state.bindings.remove(&addr);
                self.actor_by_addr.remove(&addr);
                self.allocator.dealloc_addr(&addr);
            }
        }

        false
    }

    pub fn register_stream(
        &mut self,
        stream: Stream,
        local_actor_id: ActorId,
        remote_addr: SocketAddr,
    ) -> io::Result<()> {
        let state = self.actor_states.entry(local_actor_id).or_default();
        let Some((any_addr, _)) = state.bindings.first_key_value() else {
            return Err(io::ErrorKind::AddrNotAvailable.into());
        };
        let any_addr = any_addr.clone();
        state
            .bindings
            .get_mut(&any_addr)
            .ok_or_else(|| io::ErrorKind::AddrNotAvailable)?
            .pending_outgoing
            .insert(remote_addr, stream);

        Ok(())
    }

    pub fn poll_stream(
        &mut self,
        actor_id: ActorId,
        cx: &mut Context<'_>,
    ) -> Poll<(ListenerId, SocketAddr, SocketAddr, Stream)> {
        let actor_state = self.actor_states.entry(actor_id).or_default();

        for (local_addr, local_binding) in &mut actor_state.bindings {
            let Some((addr, ())) = local_binding.pending_incoming.pop_first() else {
                continue;
            };
            let addr = addr.clone();

            let (stream, local) = Stream::pair();
            local_binding.streams.insert(addr.clone(), local);

            return Poll::Ready((local_binding.listener, local_addr.clone(), addr, stream));
        }

        actor_state.waker = Some(cx.waker().clone());

        Poll::Pending
    }

    pub fn register_msg(&mut self, dst: ActorId, _src: ActorId, msg: Msg) {
        match msg {
            Msg::Dial {
                initiator,
                responder,
            } => {
                let actor_state = self.actor_states.entry(dst).or_default();
                if let Some(binding) = actor_state.bindings.get_mut(&responder) {
                    binding.pending_incoming.insert(initiator, ());
                    actor_state.waker.take().map(Waker::wake);
                } else {
                    // TODO: report
                    // no such binding
                }
            }
            Msg::Data {
                source,
                destination,
                bytes,
            } => {
                let binding = self
                    .actor_states
                    .entry(dst)
                    .or_default()
                    .bindings
                    .get_mut(&destination)
                    .unwrap();
                let Some(stream) = binding.streams.get_mut(&source) else {
                    // TODO: log
                    return;
                };
                if stream.send(bytes).is_none() {
                    stream.mark_disconnected();
                }
            }
        }
    }

    pub fn claim_msg(&mut self, src: ActorId) -> Option<(ActorId, Msg)> {
        let actor_state = self.actor_states.entry(src).or_default();
        for (initiator, binding) in &mut actor_state.bindings {
            if let Some((responder, stream)) = binding.pending_outgoing.pop_first() {
                if let Some(dst) = self.actor_by_addr.get(&responder) {
                    binding.streams.insert(responder.clone(), stream);
                    let initiator = initiator.clone();
                    return Some((
                        *dst,
                        Msg::Dial {
                            initiator,
                            responder,
                        },
                    ));
                }
            }

            let mut to_remove = vec![];
            for (destination, stream) in &mut binding.streams {
                match stream.try_recv() {
                    Ok(bytes) => {
                        if let Some(dst) = self.actor_by_addr.get(destination) {
                            let dst = *dst;
                            let destination = destination.clone();
                            let source = initiator.clone();
                            for addr in to_remove {
                                binding.streams.remove(&addr);
                            }
                            return Some((
                                dst,
                                Msg::Data {
                                    source,
                                    destination,
                                    bytes,
                                },
                            ));
                        }
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        to_remove.push(destination.clone());
                        continue;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        continue;
                    }
                }
            }
            for addr in to_remove {
                binding.streams.remove(&addr);
            }
        }

        None
    }
}
