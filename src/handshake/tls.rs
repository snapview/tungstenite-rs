use native_tls;

use stream::Stream;
use super::{Handshake, HandshakeResult};

pub struct TlsHandshake {
    
}

impl Handshale for TlsHandshake {
    type Stream = Stream;
    fn handshake(self) -> Result<HandshakeResult<Self>> {
    }
}
