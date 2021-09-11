use std::io::Write;

use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use http::HeaderValue;
use thiserror::Error;

use crate::{
    extensions::{self, Param},
    protocol::Role,
};

const PER_MESSAGE_DEFLATE: &str = "permessage-deflate";
const CLIENT_NO_CONTEXT_TAKEOVER: &str = "client_no_context_takeover";
const SERVER_NO_CONTEXT_TAKEOVER: &str = "server_no_context_takeover";
const CLIENT_MAX_WINDOW_BITS: &str = "client_max_window_bits";
const SERVER_MAX_WINDOW_BITS: &str = "server_max_window_bits";

const TRAILER: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

/// Error from `permessage-deflate` extension.
#[derive(Debug, Error)]
pub enum DeflateError {
    /// Compress failed
    #[error("failed to compress: {0}")]
    Compress(std::io::Error),
    /// Decompress failed
    #[error("failed to decompress: {0}")]
    Decompress(std::io::Error),
}

// Parameters `server_max_window_bits` and `client_max_window_bits` are not supported for now
// because custom window size requires `flate2/zlib` feature.
// TODO Configs for how the server accepts these offers.
/// Configurations for `permessage-deflate` Per-Message Compression Extension.
#[derive(Clone, Copy, Debug)]
pub struct DeflateConfig {
    /// Compression level.
    pub compression: Compression,
    /// Request the peer server not to use context takeover.
    pub server_no_context_takeover: bool,
    /// Hint that context takeover is not used.
    pub client_no_context_takeover: bool,
}

impl Default for DeflateConfig {
    fn default() -> Self {
        Self {
            compression: Compression::default(),
            server_no_context_takeover: false,
            client_no_context_takeover: false,
        }
    }
}

impl DeflateConfig {
    pub(crate) fn name(&self) -> &str {
        PER_MESSAGE_DEFLATE
    }

    /// Value for `Sec-WebSocket-Extensions` request header.
    pub(crate) fn negotiation_offers(&self) -> HeaderValue {
        let mut offers = Vec::new();
        if self.server_no_context_takeover {
            offers.push(Param::new(SERVER_NO_CONTEXT_TAKEOVER));
        }
        if self.client_no_context_takeover {
            offers.push(Param::new(CLIENT_NO_CONTEXT_TAKEOVER));
        }
        to_header_value(&offers)
    }

    // This can be used for `WebSocket::from_raw_socket_with_compression`.
    /// Returns negotiation response based on offers and `DeflateContext` to manage per message compression.
    pub fn negotiation_response(&self, extensions: &str) -> Option<(HeaderValue, DeflateContext)> {
        // Accept the first valid offer for `permessage-deflate`.
        // A server MUST decline an extension negotiation offer for this
        // extension if any of the following conditions are met:
        // * The negotiation offer contains an extension parameter not defined
        //   for use in an offer.
        // * The negotiation offer contains an extension parameter with an
        //   invalid value.
        // * The negotiation offer contains multiple extension parameters with
        //   the same name.
        // * The server doesn't support the offered configuration.
        'outer: for (_, offer) in
            extensions::parse_header(extensions).iter().filter(|(k, _)| k == self.name())
        {
            let mut config =
                DeflateConfig { compression: self.compression, ..DeflateConfig::default() };
            let mut agreed = Vec::new();
            let mut seen_server_no_context_takeover = false;
            let mut seen_client_no_context_takeover = false;
            let mut seen_client_max_window_bits = false;
            for param in offer {
                match param.name() {
                    SERVER_NO_CONTEXT_TAKEOVER => {
                        // Invalid offer with multiple params with same name is declined.
                        if seen_server_no_context_takeover {
                            continue 'outer;
                        }
                        seen_server_no_context_takeover = true;
                        config.server_no_context_takeover = true;
                        agreed.push(Param::new(SERVER_NO_CONTEXT_TAKEOVER));
                    }

                    CLIENT_NO_CONTEXT_TAKEOVER => {
                        // Invalid offer with multiple params with same name is declined.
                        if seen_client_no_context_takeover {
                            continue 'outer;
                        }
                        seen_client_no_context_takeover = true;
                        config.client_no_context_takeover = true;
                        agreed.push(Param::new(CLIENT_NO_CONTEXT_TAKEOVER));
                    }

                    // Max window bits are not supported at the moment.
                    SERVER_MAX_WINDOW_BITS => {
                        // A server declines an extension negotiation offer with this parameter
                        // if the server doesn't support it.
                        continue 'outer;
                    }
                    // Not supported, but server may ignore and accept the offer.
                    CLIENT_MAX_WINDOW_BITS => {
                        // Invalid offer with multiple params with same name is declined.
                        if seen_client_max_window_bits {
                            continue 'outer;
                        }
                        seen_client_max_window_bits = true;
                    }

                    // Offer with unknown parameter MUST be declined.
                    _ => {
                        continue 'outer;
                    }
                }
            }

            return Some((to_header_value(&agreed), DeflateContext::new(Role::Server, config)));
        }

        None
    }

    pub(crate) fn accept_response(&self, agreed: &[Param]) -> Result<DeflateContext, DeflateError> {
        let mut config =
            DeflateConfig { compression: self.compression, ..DeflateConfig::default() };
        for param in agreed {
            match param.name() {
                SERVER_NO_CONTEXT_TAKEOVER => {
                    config.server_no_context_takeover = true;
                }

                CLIENT_NO_CONTEXT_TAKEOVER => {
                    config.client_no_context_takeover = true;
                }

                SERVER_MAX_WINDOW_BITS => {}
                CLIENT_MAX_WINDOW_BITS => {}

                _ => {
                    //
                }
            }
        }
        Ok(DeflateContext::new(Role::Client, config))
    }
}

#[derive(Debug)]
/// Manages per message compression using DEFLATE.
pub struct DeflateContext {
    role: Role,
    config: DeflateConfig,
    compressor: Compress,
    decompressor: Decompress,
}

impl DeflateContext {
    fn new(role: Role, config: DeflateConfig) -> Self {
        DeflateContext {
            role,
            config,
            compressor: Compress::new(config.compression, false),
            decompressor: Decompress::new(false),
        }
    }

    fn own_context_takeover(&self) -> bool {
        match self.role {
            Role::Server => !self.config.server_no_context_takeover,
            Role::Client => !self.config.client_no_context_takeover,
        }
    }

    fn peer_context_takeover(&self) -> bool {
        match self.role {
            Role::Server => !self.config.client_no_context_takeover,
            Role::Client => !self.config.server_no_context_takeover,
        }
    }

    // Compress the data of message.
    pub(crate) fn compress(&mut self, data: &[u8]) -> Result<Vec<u8>, DeflateError> {
        // https://datatracker.ietf.org/doc/html/rfc7692#section-7.2.1
        // 1. Compress all the octets of the payload of the message using DEFLATE.
        let mut output = Vec::with_capacity(data.len());
        let before_in = self.compressor.total_in() as usize;
        while (self.compressor.total_in() as usize) - before_in < data.len() {
            let offset = (self.compressor.total_in() as usize) - before_in;
            match self
                .compressor
                .compress_vec(&data[offset..], &mut output, FlushCompress::None)
                .map_err(|e| DeflateError::Compress(e.into()))?
            {
                Status::Ok => continue,
                Status::BufError => output.reserve(4096),
                Status::StreamEnd => break,
            }
        }
        // 2. If the resulting data does not end with an empty DEFLATE block
        //    with no compression (the "BTYPE" bits are set to 00), append an
        //    empty DEFLATE block with no compression to the tail end.
        while !output.ends_with(&TRAILER) {
            output.reserve(5);
            match self
                .compressor
                .compress_vec(&[], &mut output, FlushCompress::Sync)
                .map_err(|e| DeflateError::Compress(e.into()))?
            {
                Status::Ok | Status::BufError => continue,
                Status::StreamEnd => break,
            }
        }
        // 3. Remove 4 octets (that are 0x00 0x00 0xff 0xff) from the tail end.
        //    After this step, the last octet of the compressed data contains
        //    (possibly part of) the DEFLATE header bits with the "BTYPE" bits
        //    set to 00.
        output.truncate(output.len() - 4);

        if !self.own_context_takeover() {
            self.compressor.reset();
        }

        Ok(output)
    }

    pub(crate) fn decompress(
        &mut self,
        mut data: Vec<u8>,
        is_final: bool,
    ) -> Result<Vec<u8>, DeflateError> {
        if is_final {
            data.extend_from_slice(&TRAILER);
        }

        let before_in = self.decompressor.total_in() as usize;
        let mut output = Vec::with_capacity(2 * data.len());
        loop {
            let offset = (self.decompressor.total_in() as usize) - before_in;
            match self
                .decompressor
                .decompress_vec(&data[offset..], &mut output, FlushDecompress::None)
                .map_err(|e| DeflateError::Decompress(e.into()))?
            {
                Status::Ok => output.reserve(2 * output.len()),
                Status::BufError | Status::StreamEnd => break,
            }
        }

        if is_final && !self.peer_context_takeover() {
            self.decompressor.reset(false);
        }

        Ok(output)
    }
}

fn to_header_value(params: &[Param]) -> HeaderValue {
    let mut value = Vec::new();
    write!(value, "{}", PER_MESSAGE_DEFLATE).unwrap();
    for param in params {
        if let Some(v) = param.value() {
            write!(value, "; {}={}", param.name(), v).unwrap();
        } else {
            write!(value, "; {}", param.name()).unwrap();
        }
    }
    HeaderValue::from_bytes(&value).unwrap()
}
