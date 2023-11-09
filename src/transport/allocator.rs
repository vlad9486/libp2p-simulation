use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, IpAddr},
    fmt,
};

pub struct Allocator {
    address_bitmap: [u8; 0x2000],
}

impl fmt::Debug for Allocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Allocator").finish()
    }
}

impl Default for Allocator {
    fn default() -> Self {
        Allocator {
            address_bitmap: [0; 0x2000],
        }
    }
}

impl Allocator {
    pub fn allocate_addr(&mut self, addr: SocketAddr) -> Option<SocketAddr> {
        let ip = match addr.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
            IpAddr::V6(ip) if ip.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
            v => v,
        };
        let port = if addr.port() == 0 {
            self.alloc_port()?
        } else {
            if !self.mark_port(addr.port()) {
                self.mark_port(addr.port());
                return None;
            }
            addr.port()
        };
        Some(SocketAddr::new(ip, port))
    }

    pub fn dealloc_addr(&mut self, addr: &SocketAddr) {
        self.dealloc_port(addr.port());
    }

    /// returns true if port was free
    fn mark_port(&mut self, port: u16) -> bool {
        let i = (port / 8) as usize;
        let j = port % 8;
        self.address_bitmap[i] ^= 1 << j;
        self.address_bitmap[i] & (1 << j) != 0
    }

    fn alloc_port(&mut self) -> Option<u16> {
        let (i, bitmap) = self.address_bitmap[0x80..]
            .iter_mut()
            .enumerate()
            .find(|(_, b)| **b != 0xff)?;
        let j = (0..8u16)
            .find(|j| *bitmap & (1 << j) == 0)
            .expect("must be there");
        *bitmap |= 1 << j;

        Some((i * 8) as u16 + j + 1024)
    }

    fn dealloc_port(&mut self, port: u16) {
        assert!(port >= 1024);
        assert!(!self.mark_port(port));
    }
}
