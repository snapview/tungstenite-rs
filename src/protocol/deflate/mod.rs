use flate2::Compression;
use http::{HeaderMap, HeaderValue};

use crate::{error::Result, protocol::Role};

mod codec;
mod negotiate;

pub(crate) use self::{codec::Context, negotiate::headers_select_deflate};

const NAME: &str = "permessage-deflate";

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) compression: Compression,
    pub(crate) server_no_context_takeover: bool,
    pub(crate) client_no_context_takeover: bool,
    pub(crate) server_max_window_bits: u8,
    pub(crate) client_max_window_bits: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            compression: Compression::default(),
            server_no_context_takeover: false,
            client_no_context_takeover: false,
            server_max_window_bits: 15,
            client_max_window_bits: 15,
        }
    }
}

impl Settings {
    pub(crate) fn max_window_bits(mut self, role: Role, bits: u8) -> Self {
        assert!((9..=15).contains(&bits), "deflate window bits must be in 9..=15");
        *match role {
            Role::Server => &mut self.server_max_window_bits,
            Role::Client => &mut self.client_max_window_bits,
        } = bits;
        self
    }

    pub(crate) fn no_context_takeover(mut self, role: Role, on: bool) -> Self {
        *match role {
            Role::Server => &mut self.server_no_context_takeover,
            Role::Client => &mut self.client_no_context_takeover,
        } = on;
        self
    }

    pub(crate) fn compression_level(mut self, level: u32) -> Self {
        assert!(level <= 9, "deflate compression level must be in 0..=9");
        self.compression = Compression::new(level);
        self
    }

    pub(crate) fn offer(self) -> HeaderValue {
        negotiate::offer(self)
    }

    pub(crate) fn accept_response(self, headers: &HeaderMap) -> Result<Option<Self>> {
        negotiate::accept_response(self, headers)
    }

    pub(crate) fn accept_offers(self, offers: &[HeaderValue]) -> Option<(Self, HeaderValue)> {
        negotiate::accept_offers(self, offers)
    }
}
