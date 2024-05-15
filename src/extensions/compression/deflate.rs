use std::convert::TryFrom;

use bytes::BytesMut;
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use headers::WebsocketExtension;
use http::HeaderValue;
use thiserror::Error;

use crate::protocol::Role;

const PER_MESSAGE_DEFLATE: &str = "permessage-deflate";
const CLIENT_NO_CONTEXT_TAKEOVER: &str = "client_no_context_takeover";
const SERVER_NO_CONTEXT_TAKEOVER: &str = "server_no_context_takeover";
const CLIENT_MAX_WINDOW_BITS: &str = "client_max_window_bits";
const SERVER_MAX_WINDOW_BITS: &str = "server_max_window_bits";

const TRAILER: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

/// Errors from `permessage-deflate` extension.
#[derive(Debug, Error)]
pub enum DeflateError {
    /// Compress failed
    #[error("Failed to compress")]
    Compress(#[source] std::io::Error),
    /// Decompress failed
    #[error("Failed to decompress")]
    Decompress(#[source] std::io::Error),

    /// Extension negotiation failed.
    #[error("Extension negotiation failed")]
    Negotiation(#[source] NegotiationError),
}

/// Errors from `permessage-deflate` extension negotiation.
#[derive(Debug, Error)]
pub enum NegotiationError {
    /// Unknown parameter in a negotiation response.
    #[error("Unknown parameter in a negotiation response: {0}")]
    UnknownParameter(String),
    /// Duplicate parameter in a negotiation response.
    #[error("Duplicate parameter in a negotiation response: {0}")]
    DuplicateParameter(String),
    /// Received `client_max_window_bits` in a negotiation response for an offer without it.
    #[error("Received client_max_window_bits in a negotiation response for an offer without it")]
    UnexpectedClientMaxWindowBits,
    /// Received unsupported `server_max_window_bits` in a negotiation response.
    #[error("Received unsupported server_max_window_bits in a negotiation response")]
    ServerMaxWindowBitsNotSupported,
    /// Invalid `client_max_window_bits` value in a negotiation response.
    #[error("Invalid client_max_window_bits value in a negotiation response: {0}")]
    InvalidClientMaxWindowBitsValue(String),
    /// Invalid `server_max_window_bits` value in a negotiation response.
    #[error("Invalid server_max_window_bits value in a negotiation response: {0}")]
    InvalidServerMaxWindowBitsValue(String),
    /// Missing `server_max_window_bits` value in a negotiation response.
    #[error("Missing server_max_window_bits value in a negotiation response")]
    MissingServerMaxWindowBitsValue,
}

// Parameters `server_max_window_bits` and `client_max_window_bits` are not supported for now
// because custom window size requires `flate2/zlib` feature.
/// Configurations for `permessage-deflate` Per-Message Compression Extension.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeflateConfig {
    /// Compression level.
    pub compression: Compression,
    /// Request the peer server not to use context takeover.
    pub server_no_context_takeover: bool,
    /// Hint that context takeover is not used.
    pub client_no_context_takeover: bool,
}

impl DeflateConfig {
    pub(crate) fn name(&self) -> &str {
        PER_MESSAGE_DEFLATE
    }

    /// Value for `Sec-WebSocket-Extensions` request header.
    pub(crate) fn generate_offer(&self) -> WebsocketExtension {
        let mut offers = Vec::new();
        if self.server_no_context_takeover {
            offers.push(HeaderValue::from_static(SERVER_NO_CONTEXT_TAKEOVER));
        }

        // > a client informs the peer server of a hint that even if the server doesn't include the
        // > "client_no_context_takeover" extension parameter in the corresponding
        // > extension negotiation response to the offer, the client is not going
        // > to use context takeover.
        // > https://www.rfc-editor.org/rfc/rfc7692#section-7.1.1.2
        if self.client_no_context_takeover {
            offers.push(HeaderValue::from_static(CLIENT_NO_CONTEXT_TAKEOVER));
        }
        to_header_value(&offers)
    }

    /// Returns negotiation response based on offers and `DeflateContext` to manage per message compression.
    pub(crate) fn accept_offer(
        &self,
        offers: &headers::SecWebsocketExtensions,
    ) -> Option<(WebsocketExtension, DeflateContext)> {
        // Accept the first valid offer for `permessage-deflate`.
        // A server MUST decline an extension negotiation offer for this
        // extension if any of the following conditions are met:
        // 1. The negotiation offer contains an extension parameter not defined for use in an offer.
        // 2. The negotiation offer contains an extension parameter with an invalid value.
        // 3. The negotiation offer contains multiple extension parameters with the same name.
        // 4. The server doesn't support the offered configuration.
        offers.iter().find_map(|extension| {
            if let Some(params) = (extension.name() == self.name()).then(|| extension.params()) {
                let mut config =
                    DeflateConfig { compression: self.compression, ..DeflateConfig::default() };
                let mut agreed = Vec::new();
                let mut seen_server_no_context_takeover = false;
                let mut seen_client_no_context_takeover = false;
                let mut seen_client_max_window_bits = false;
                for (key, val) in params {
                    match key {
                        SERVER_NO_CONTEXT_TAKEOVER => {
                            // Invalid offer with multiple params with same name is declined.
                            if seen_server_no_context_takeover {
                                return None;
                            }
                            seen_server_no_context_takeover = true;
                            config.server_no_context_takeover = true;
                            agreed.push(HeaderValue::from_static(SERVER_NO_CONTEXT_TAKEOVER));
                        }

                        CLIENT_NO_CONTEXT_TAKEOVER => {
                            // Invalid offer with multiple params with same name is declined.
                            if seen_client_no_context_takeover {
                                return None;
                            }
                            seen_client_no_context_takeover = true;
                            config.client_no_context_takeover = true;
                            agreed.push(HeaderValue::from_static(CLIENT_NO_CONTEXT_TAKEOVER));
                        }

                        // Max window bits are not supported at the moment.
                        SERVER_MAX_WINDOW_BITS => {
                            // Decline offer with invalid parameter value.
                            // `server_max_window_bits` requires a value in range [8, 15].
                            if let Some(bits) = val {
                                if !is_valid_max_window_bits(bits) {
                                    return None;
                                }
                            } else {
                                return None;
                            }

                            // A server declines an extension negotiation offer with this parameter
                            // if the server doesn't support it.
                            return None;
                        }

                        // Not supported, but server may ignore and accept the offer.
                        CLIENT_MAX_WINDOW_BITS => {
                            // Decline offer with invalid parameter value.
                            // `client_max_window_bits` requires a value in range [8, 15] or no value.
                            if let Some(bits) = val {
                                if !is_valid_max_window_bits(bits) {
                                    return None;
                                }
                            }

                            // Invalid offer with multiple params with same name is declined.
                            if seen_client_max_window_bits {
                                return None;
                            }
                            seen_client_max_window_bits = true;
                        }

                        // Offer with unknown parameter MUST be declined.
                        _ => {
                            return None;
                        }
                    }
                }

                Some((to_header_value(&agreed), DeflateContext::new(Role::Server, config)))
            } else {
                None
            }
        })
    }

    pub(crate) fn accept_response<'a>(
        &'a self,
        agreed: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    ) -> Result<DeflateContext, DeflateError> {
        let mut config = DeflateConfig {
            compression: self.compression,
            // If this was hinted in the offer, the client won't use context takeover
            // even if the response doesn't include it.
            // See `generate_offer`.
            client_no_context_takeover: self.client_no_context_takeover,
            ..DeflateConfig::default()
        };
        let mut seen_server_no_context_takeover = false;
        let mut seen_client_no_context_takeover = false;
        // A client MUST _Fail the WebSocket Connection_ if the peer server
        // accepted an extension negotiation offer for this extension with an
        // extension negotiation response meeting any of the following
        // conditions:
        // 1. The negotiation response contains an extension parameter not defined for use in a response.
        // 2. The negotiation response contains an extension parameter with an invalid value.
        // 3. The negotiation response contains multiple extension parameters with the same name.
        // 4. The client does not support the configuration that the response represents.
        for (key, val) in agreed {
            match key {
                SERVER_NO_CONTEXT_TAKEOVER => {
                    // Fail the connection when the response contains multiple parameters with the same name.
                    if seen_server_no_context_takeover {
                        return Err(DeflateError::Negotiation(
                            NegotiationError::DuplicateParameter(key.to_owned()),
                        ));
                    }
                    seen_server_no_context_takeover = true;
                    // A server MAY include the "server_no_context_takeover" extension
                    // parameter in an extension negotiation response even if the extension
                    // negotiation offer being accepted by the extension negotiation
                    // response didn't include the "server_no_context_takeover" extension
                    // parameter.
                    config.server_no_context_takeover = true;
                }

                CLIENT_NO_CONTEXT_TAKEOVER => {
                    // Fail the connection when the response contains multiple parameters with the same name.
                    if seen_client_no_context_takeover {
                        return Err(DeflateError::Negotiation(
                            NegotiationError::DuplicateParameter(key.to_owned()),
                        ));
                    }
                    seen_client_no_context_takeover = true;
                    // The server may include this parameter in the response and the client MUST support it.
                    config.client_no_context_takeover = true;
                }

                SERVER_MAX_WINDOW_BITS => {
                    // Fail the connection when the response contains a parameter with invalid value.
                    if let Some(bits) = val {
                        if !is_valid_max_window_bits(bits) {
                            return Err(DeflateError::Negotiation(
                                NegotiationError::InvalidServerMaxWindowBitsValue(bits.to_owned()),
                            ));
                        }
                    } else {
                        return Err(DeflateError::Negotiation(
                            NegotiationError::MissingServerMaxWindowBitsValue,
                        ));
                    }

                    // A server may include the "server_max_window_bits" extension parameter
                    // in an extension negotiation response even if the extension
                    // negotiation offer being accepted by the response didn't include the
                    // "server_max_window_bits" extension parameter.
                    //
                    // However, but we need to fail the connection because we don't support it (condition 4).
                    return Err(DeflateError::Negotiation(
                        NegotiationError::ServerMaxWindowBitsNotSupported,
                    ));
                }

                CLIENT_MAX_WINDOW_BITS => {
                    // Fail the connection when the response contains a parameter with invalid value.
                    if let Some(bits) = val {
                        if !is_valid_max_window_bits(bits) {
                            return Err(DeflateError::Negotiation(
                                NegotiationError::InvalidClientMaxWindowBitsValue(bits.to_owned()),
                            ));
                        }
                    }

                    // Fail the connection because the parameter is invalid when the client didn't offer.
                    //
                    // If a received extension negotiation offer doesn't have the
                    // "client_max_window_bits" extension parameter, the corresponding
                    // extension negotiation response to the offer MUST NOT include the
                    // "client_max_window_bits" extension parameter.
                    return Err(DeflateError::Negotiation(
                        NegotiationError::UnexpectedClientMaxWindowBits,
                    ));
                }

                // Response with unknown parameter MUST fail the WebSocket connection.
                _ => {
                    return Err(DeflateError::Negotiation(NegotiationError::UnknownParameter(
                        key.to_owned(),
                    )));
                }
            }
        }
        Ok(DeflateContext::new(Role::Client, config))
    }
}

// A valid `client_max_window_bits` is no value or an integer in range `[8, 15]` without leading zeros.
// A valid `server_max_window_bits` is an integer in range `[8, 15]` without leading zeros.
fn is_valid_max_window_bits(bits: &str) -> bool {
    // Note that values from `headers::SecWebSocketExtensions` is unquoted.
    matches!(bits, "8" | "9" | "10" | "11" | "12" | "13" | "14" | "15")
}

#[cfg(test)]
mod tests {
    use super::is_valid_max_window_bits;

    #[test]
    fn valid_max_window_bits() {
        for bits in 8..=15 {
            assert!(is_valid_max_window_bits(&bits.to_string()));
        }
    }

    #[test]
    fn invalid_max_window_bits() {
        assert!(!is_valid_max_window_bits(""));
        assert!(!is_valid_max_window_bits("0"));
        assert!(!is_valid_max_window_bits("08"));
        assert!(!is_valid_max_window_bits("+8"));
        assert!(!is_valid_max_window_bits("-8"));
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

fn to_header_value(params: &[HeaderValue]) -> WebsocketExtension {
    let mut buf = BytesMut::from(PER_MESSAGE_DEFLATE.as_bytes());
    for param in params {
        buf.extend_from_slice(b"; ");
        buf.extend_from_slice(param.as_bytes());
    }
    let header = HeaderValue::from_maybe_shared(buf.freeze())
        .expect("semicolon separated HeaderValue is valid");
    WebsocketExtension::try_from(header).expect("valid extension")
}
