use std::{
    borrow::Cow,
    future::Future,
    sync::{Mutex, Arc},
    task::{Context, Poll},
    pin::Pin,
    ops::{Deref, DerefMut},
};

use libp2p_core::{muxing::StreamMuxerBox, transport::Boxed};
use libp2p_identity::PeerId;
use libp2p_swarm::{NetworkBehaviour, Swarm, SwarmEvent, THandlerErr, Config};

use cooked_waker::{WakeRef, IntoWaker};
use futures::{StreamExt, stream::FuturesUnordered};

use super::transport::{Back, Msg, Front, Registry, ActorId};

pub struct Application<S>
where
    S: SwarmActor,
{
    config: S::Config,
    // TODO: move `transport` in state
    transport: Back,
    // TODO: move `swarm` in state, probably, it is impossible without rewriting libp2p
    swarm: Mutex<Swarm<S::Behaviour>>,
    waker: Arc<waker::Custom>,
    pool: Arc<Mutex<FuturesUnordered<Task>>>,
}

// currently the application state is just a swarm actor,
// but we should add here transport and behaviour,
// keeping traits (Debug, Clone, Eq, Hash) for `stateright`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct State<S>(S);

impl<S> Deref for State<S> {
    type Target = S;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> DerefMut for State<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

impl<S> Application<S>
where
    S: SwarmActor,
    State<S>: Clone,
{
    pub fn new<F>(
        registry: Arc<Mutex<Registry>>,
        local_peer_id: PeerId,
        config: S::Config,
        transport_factory: F,
        behaviour: S::Behaviour,
    ) -> Self
    where
        F: Fn(Front) -> Boxed<(PeerId, StreamMuxerBox)>,
    {
        let (transport, front) = Front::new(registry);

        let pool = Arc::new(Mutex::new(FuturesUnordered::default()));

        let swarm = Swarm::new(
            transport_factory(front),
            behaviour,
            local_peer_id,
            Config::with_executor({
                let pool = pool.clone();
                move |task| pool.lock().expect("poisoned").push(task)
            }),
        );

        Application {
            config,
            transport,
            swarm: Mutex::new(swarm),
            waker: Arc::new(waker::Custom::new()),
            pool,
        }
    }
}

impl<S> Application<S>
where
    S: SwarmActor,
    State<S>: Clone,
{
    fn poll_messages<F>(&self, state: &mut Cow<State<S>>, cx: &mut Context<'_>, mut o: F)
    where
        F: FnMut((ActorId, Msg)),
    {
        let mut swarm = self.swarm.lock().expect("poisoned");
        while let Poll::Ready(Some(event)) = swarm.poll_next_unpin(cx) {
            state.to_mut().0.on_event(&mut *swarm, event);
        }
        drop(swarm);

        let mut pool = self.pool.lock().expect("poisoned");
        while let Poll::Ready(Some(())) = pool.poll_next_unpin(cx) {}

        while let Some(v) = self.transport.to_send() {
            o(v);
        }
    }

    pub fn on_start<F>(&self, id: ActorId, mut f: F) -> State<S>
    where
        F: FnMut((ActorId, Msg)),
    {
        self.transport.init(id.into());

        let mut swarm = self.swarm.lock().expect("poisoned");
        let actor = S::init(&self.config, &mut *swarm);
        drop(swarm);

        let mut state = Cow::Owned(State(actor));

        self.waker.wake_by_ref();
        let waker = self.waker.clone().into_waker();
        let mut cx = Context::from_waker(&waker);

        while self.waker.should_wake() {
            self.poll_messages(&mut state, &mut cx, &mut f)
        }

        state.into_owned()
    }

    pub fn on_msg<F>(
        &self,
        id: ActorId,
        state: &mut Cow<State<S>>,
        src: ActorId,
        msg: Msg,
        mut f: F,
    ) where
        F: FnMut((ActorId, Msg)),
    {
        self.transport.on_msg(src.into(), msg);

        self.waker.wake_by_ref();
        let waker = self.waker.clone().into_waker();
        let mut cx = Context::from_waker(&waker);

        log::info!("{id} processing messages");
        while self.waker.should_wake() {
            self.poll_messages(state, &mut cx, &mut f);
        }
    }

    pub fn run<I>(iter: I)
    where
        I: IntoIterator<Item = Self>,
    {
        use std::{collections::BTreeMap, mem};

        let mut actors = iter
            .into_iter()
            .map(|app| (app, None::<Cow<'_, State<S>>>))
            .collect::<Vec<_>>();
        let mut messages = BTreeMap::new();
        let mut new_messages = BTreeMap::<_, Vec<(ActorId, Msg)>>::new();

        loop {
            for (n, (actor, state)) in actors.iter_mut().enumerate() {
                let id = n.into();
                match state {
                    None => {
                        let mut out = vec![];
                        *state = Some(Cow::Owned(actor.on_start(id, |v| out.push(v))));
                        for (dst, msg) in out {
                            log::info!("{id} -> {dst}: {msg:?}");
                            new_messages.entry(dst).or_default().push((id, msg))
                        }
                    }
                    Some(state) => {
                        if let Some(list) = messages.remove(&id) {
                            let mut out = vec![];
                            for (src, msg) in list {
                                actor.on_msg(id, state, src, msg, |v| out.push(v));
                            }
                            for (dst, msg) in out {
                                log::info!("{id} -> {dst}: {msg:?}");
                                new_messages.entry(dst).or_default().push((id, msg))
                            }
                        }
                    }
                }
            }

            mem::swap(&mut messages, &mut new_messages);
            new_messages.clear();

            if messages.is_empty() {
                break;
            }
        }
    }
}

pub trait SwarmActor {
    type Behaviour: NetworkBehaviour;

    type Config;

    fn init(config: &Self::Config, swarm: &mut Swarm<Self::Behaviour>) -> Self;

    fn on_event(
        &mut self,
        swarm: &mut Swarm<Self::Behaviour>,
        event: SwarmEvent<
            <Self::Behaviour as NetworkBehaviour>::ToSwarm,
            THandlerErr<Self::Behaviour>,
        >,
    );
}

mod waker {
    use cooked_waker::{Wake, WakeRef};
    use std::sync::atomic::{AtomicBool, Ordering};

    pub struct Custom {
        will_wake: AtomicBool,
    }

    impl Custom {
        pub fn new() -> Self {
            Custom {
                will_wake: AtomicBool::new(false),
            }
        }

        pub fn should_wake(&self) -> bool {
            self.will_wake.swap(false, Ordering::SeqCst)
        }
    }

    impl WakeRef for Custom {
        fn wake_by_ref(&self) {
            self.will_wake.store(true, Ordering::SeqCst);
        }
    }

    impl Wake for Custom {
        fn wake(self) {
            self.will_wake.store(true, Ordering::SeqCst);
        }
    }

    impl Drop for Custom {
        fn drop(&mut self) {}
    }
}

#[cfg(feature = "stateright")]
mod stateright {
    use std::{fmt::Debug, hash::Hash, borrow::Cow};

    use stateright::actor;

    use super::{Application, SwarmActor, Msg, State};

    impl<S> actor::Actor for Application<S>
    where
        S: SwarmActor,
        State<S>: Debug + Clone + Eq + Hash,
    {
        type Msg = Msg;
        type State = State<S>;
        type Timer = ();

        fn on_start(&self, id: actor::Id, o: &mut actor::Out<Self>) -> Self::State {
            Self::on_start(&self, id.into(), |(dst, msg)| o.send(dst.into(), msg))
        }

        fn on_msg(
            &self,
            id: actor::Id,
            state: &mut Cow<Self::State>,
            src: actor::Id,
            msg: Self::Msg,
            o: &mut actor::Out<Self>,
        ) {
            Self::on_msg(&self, id.into(), state, src.into(), msg, |(dst, msg)| {
                o.send(dst.into(), msg)
            })
        }
    }
}
