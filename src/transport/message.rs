extern crate alloc;

use std::{fmt, net::SocketAddr};

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Msg {
    Dial {
        initiator: SocketAddr,
        responder: SocketAddr,
    },
    Data {
        source: SocketAddr,
        destination: SocketAddr,
        bytes: Vec<u8>,
    },
}

impl fmt::Debug for Msg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dial {
                initiator,
                responder,
            } => f
                .debug_tuple("Dial")
                .field(&initiator.to_string())
                .field(&responder.to_string())
                .finish(),
            Self::Data {
                source,
                destination,
                bytes,
            } => f
                .debug_tuple("Data")
                .field(&source.to_string())
                .field(&destination.to_string())
                .field(&hex::encode(bytes))
                .finish(),
        }
    }
}
