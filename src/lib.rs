#[cfg(test)]
mod tests;

mod transport;
pub use self::transport::{Registry, Msg, ActorId, Stream, Front};

mod application;
pub use self::application::{Application, SwarmActor};
