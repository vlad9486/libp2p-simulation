use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use super::{state::Registry, message::Msg};

pub type ActorId = usize;

#[derive(Clone, Default)]
pub struct ActorIdCell(Arc<AtomicUsize>);

impl ActorIdCell {
    pub fn put(&self, id: ActorId) {
        self.0.store(id + 1, Ordering::SeqCst);
    }

    pub fn get(&self) -> Option<ActorId> {
        let v = self.0.load(Ordering::Relaxed);
        if v == 0 {
            None
        } else {
            Some(v - 1)
        }
    }
}

pub struct Back {
    id: ActorIdCell,
    registry: Arc<Mutex<Registry>>,
}

impl Back {
    pub(super) fn new(registry: Arc<Mutex<Registry>>, id: ActorIdCell) -> Self {
        Back { id, registry }
    }

    pub fn init(&self, id: ActorId) {
        if let Some(old_id) = self.id.get() {
            self.registry.lock().expect("poisoned").cleanup(old_id);
        }
        self.id.put(id);
    }

    pub fn on_msg(&self, src: ActorId, msg: Msg) {
        let dst = self.id.get().expect("must be initialized");
        self.registry
            .lock()
            .expect("poisoned")
            .register_msg(dst, src, msg);
    }

    pub fn to_send(&self) -> Option<(ActorId, Msg)> {
        let src = self.id.get().expect("must be initialized");
        let mut lock = self.registry.lock().expect("poisoned");

        lock.claim_msg(src)
    }
}
