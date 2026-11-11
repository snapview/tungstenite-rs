use bytes::Bytes;
use flate2::{Compress, Decompress, FlushCompress, FlushDecompress, Status};

use super::Settings;
use crate::{
    error::{ProtocolError, Result},
    protocol::Role,
};

#[derive(Debug)]
pub(crate) struct Context {
    encoder: Compress,
    decoder: ConfiguredDecoder,
    reset_encoder: bool,
    reset_decoder: bool,
}

/// An inflater that remembers the window it was built with.
///
/// `Decompress::reset` takes no width and restores a 15-bit window on every backend flate2
/// admits, so resetting a negotiated narrower decoder silently widens it. Reconstructing keeps
/// the memory reduction those parameters exist for -- RFC 7692 §7.1.2.1 for a client's decoder,
/// §7.1.2.2 for a server's -- across the whole connection, not until the first reset.
#[derive(Debug)]
struct ConfiguredDecoder {
    stream: Decompress,
    window_bits: u8,
}

/// A 15-bit inflater is what `Decompress::reset` produces anyway, so only a narrower one
/// has to be rebuilt. Keeping the rule here rather than at the reset sites means both of
/// them share it.
fn reconstructs_on_reset(window_bits: u8) -> bool {
    window_bits < 15
}

impl ConfiguredDecoder {
    fn new(role: Role, settings: Settings) -> Self {
        let peer_window = match role {
            Role::Client => settings.server_max_window_bits,
            Role::Server => settings.client_max_window_bits,
        };
        // flate2 asserts 9..=15 in both constructors and RFC 7692 permits a negotiated 8, which
        // neither accept path bounds from below -- hence the clamp. Inflating wider than the
        // peer encoded with is always safe.
        let window_bits = peer_window.max(9);
        Self { stream: Decompress::new_with_window_bits(false, window_bits), window_bits }
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<(Status, usize, usize)> {
        let before = (self.stream.total_in(), self.stream.total_out());
        let status = self
            .stream
            .decompress(input, output, FlushDecompress::None)
            .map_err(|_| ProtocolError::Compression)?;
        let consumed = (self.stream.total_in() - before.0) as usize;
        let produced = (self.stream.total_out() - before.1) as usize;
        Ok((status, consumed, produced))
    }

    fn reset(&mut self) {
        if reconstructs_on_reset(self.window_bits) {
            self.stream = Decompress::new_with_window_bits(false, self.window_bits);
        } else {
            self.stream.reset(false);
        }
    }
}

impl Context {
    pub(crate) fn new(role: Role, settings: Settings) -> Self {
        let (own_window, reset_encoder, reset_decoder) = match role {
            Role::Client => (
                settings.client_max_window_bits,
                settings.client_no_context_takeover,
                settings.server_no_context_takeover,
            ),
            Role::Server => (
                settings.server_max_window_bits,
                settings.server_no_context_takeover,
                settings.client_no_context_takeover,
            ),
        };
        // An own window of 8 is refused during negotiation, in `accept_response` (client)
        // and `accept_offer` (server), so the encoder needs no clamp -- and must not
        // inherit the decoder's, because compressing wider than was negotiated emits
        // backreferences the peer cannot resolve.
        Self {
            encoder: Compress::new_with_window_bits(settings.compression, false, own_window),
            decoder: ConfiguredDecoder::new(role, settings),
            reset_encoder,
            reset_decoder,
        }
    }

    pub(crate) fn reset_encoder(&mut self) {
        self.encoder.reset();
    }

    pub(crate) fn compress(&mut self, mut input: &[u8]) -> Result<Bytes> {
        const CHUNK: usize = 4096;
        const TRAILER: &[u8] = &[0, 0, 0xff, 0xff];
        if input.is_empty() {
            return Ok(Bytes::from_static(&[0]));
        }
        let mut output = Vec::new();
        while !input.is_empty() {
            output.reserve(CHUNK);
            let before = (self.encoder.total_in(), self.encoder.total_out());
            let status = self
                .encoder
                .compress_vec(input, &mut output, FlushCompress::None)
                .map_err(|_| ProtocolError::Compression)?;
            let consumed = (self.encoder.total_in() - before.0) as usize;
            let produced = (self.encoder.total_out() - before.1) as usize;
            input = &input[consumed..];
            if !progress(status, consumed, produced)? {
                break;
            }
        }
        loop {
            output.reserve(CHUNK);
            let before = self.encoder.total_out();
            let status = self
                .encoder
                .compress_vec(&[], &mut output, FlushCompress::Sync)
                .map_err(|_| ProtocolError::Compression)?;
            let produced = (self.encoder.total_out() - before) as usize;
            if !progress(status, 0, produced)? {
                break;
            }
        }
        if !output.ends_with(TRAILER) {
            self.encoder.reset();
            return Err(ProtocolError::Compression.into());
        }
        output.truncate(output.len() - TRAILER.len());
        if self.reset_encoder {
            self.encoder.reset();
        }
        Ok(output.into())
    }

    pub(crate) fn decompress(
        &mut self,
        input: &[u8],
        final_frame: bool,
        already: usize,
        max_size: Option<usize>,
    ) -> Result<Bytes> {
        const TRAILER: &[u8] = &[0, 0, 0xff, 0xff];
        let max_size = max_size.unwrap_or(usize::MAX);
        let mut output = Vec::new();
        self.inflate(input, already, max_size, &mut output)?;
        if final_frame {
            self.inflate(TRAILER, already, max_size, &mut output)?;
            if self.reset_decoder {
                self.decoder.reset();
            }
        }
        Ok(output.into())
    }

    fn inflate(
        &mut self,
        mut input: &[u8],
        already: usize,
        max_size: usize,
        output: &mut Vec<u8>,
    ) -> Result<()> {
        let mut scratch = [0; 4096];
        loop {
            let remaining = max_size.saturating_sub(already.saturating_add(output.len()));
            let writable = remaining.saturating_add(1).min(scratch.len());
            let (status, consumed, produced) =
                self.decoder.decompress(input, &mut scratch[..writable])?;
            output.extend_from_slice(&scratch[..produced]);
            if already.saturating_add(output.len()) > max_size {
                return Err(crate::error::CapacityError::MessageTooLong {
                    size: already.saturating_add(output.len()),
                    max_size,
                }
                .into());
            }
            input = &input[consumed..];
            if status == Status::StreamEnd {
                self.decoder.reset();
            }
            if !progress(status, consumed, produced)? {
                return Ok(());
            }
        }
    }
}

fn progress(status: Status, consumed: usize, produced: usize) -> Result<bool> {
    if consumed != 0 || produced != 0 {
        return Ok(true);
    }
    match status {
        Status::Ok => Err(ProtocolError::Compression.into()),
        Status::BufError | Status::StreamEnd => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{CapacityError, Error};

    fn pair() -> (Context, Context) {
        (
            Context::new(Role::Server, Settings::default()),
            Context::new(Role::Client, Settings::default()),
        )
    }

    /// The decoder's width is the *peer's* agreed window, never the endpoint's own, and an
    /// agreed 8 -- legal under RFC 7692, unconstructible in flate2 -- is stored as the
    /// effective 9. Both cases set the two windows apart, because equal values would pass
    /// whichever field the mapping read.
    #[test]
    fn the_configured_decoder_stores_the_peers_effective_window() {
        let server = ConfiguredDecoder::new(
            Role::Server,
            Settings {
                server_max_window_bits: 10,
                client_max_window_bits: 15,
                ..Settings::default()
            },
        );
        assert_eq!(server.window_bits, 15);

        let client = ConfiguredDecoder::new(
            Role::Client,
            Settings {
                client_max_window_bits: 10,
                server_max_window_bits: 8,
                ..Settings::default()
            },
        );
        assert_eq!(client.window_bits, 9, "an agreed 8 is stored as the constructible 9");

        for bits in 9..=14 {
            assert!(reconstructs_on_reset(bits), "a {bits}-bit decoder must be rebuilt");
        }
        assert!(!reconstructs_on_reset(15), "a 15-bit decoder keeps the in-place reset");

        let mut client = Context::new(
            Role::Client,
            Settings { server_max_window_bits: 8, ..Settings::default() },
        );
        let mut server = Context::new(
            Role::Server,
            Settings { server_max_window_bits: 9, ..Settings::default() },
        );
        let payload = b"the quick brown fox jumps over the lazy dog, twice over";
        let compressed = server.compress(payload).expect("peer compresses");
        let inflated = client.decompress(&compressed, true, 0, None).expect("we inflate");
        assert_eq!(inflated.as_ref(), payload);
    }

    /// `inflate` detects an over-budget message by allowing exactly one byte past
    /// the budget and observing that it arrived: `writable` is
    /// `remaining.saturating_add(1)`. That `+1` is the entire detector, so the
    /// interesting rows are the two either side of the boundary -- at the limit
    /// exactly, which must be accepted, and one byte over, which must not.
    #[test]
    fn a_message_is_accepted_at_the_limit_and_rejected_one_byte_over() {
        let payload = vec![b'x'; 4000];
        let (mut server, mut client) = pair();
        let wire = server.compress(&payload).expect("compress");

        let at_limit = client.decompress(&wire, true, 0, Some(payload.len()));
        assert_eq!(
            at_limit.expect("a message exactly at the limit must be accepted").as_ref(),
            &payload[..],
            "and it must decode whole"
        );

        let (mut server, mut client) = pair();
        let wire = server.compress(&payload).expect("compress");
        match client.decompress(&wire, true, 0, Some(payload.len() - 1)) {
            Err(Error::Capacity(CapacityError::MessageTooLong { size, max_size })) => {
                // `size > max_size` would be a tautology -- the error is only
                // constructed when that holds -- and echoing `max_size` back
                // asserts an input. The invariant with content is that detection
                // happens on the first byte past the budget.
                assert_eq!(size, max_size + 1, "detection must be one byte past the budget");
            }
            other => panic!("one byte over the limit must be MessageTooLong, got {other:?}"),
        }
    }

    /// `already` is the bytes an earlier frame of the same message contributed, so
    /// the budget is per message rather than per frame. Without it a fragmented
    /// message could deliver `max_size` bytes per frame indefinitely.
    #[test]
    fn the_budget_counts_bytes_delivered_by_earlier_frames() {
        let payload = vec![b'y'; 2000];
        let (mut server, mut client) = pair();
        let wire = server.compress(&payload).expect("compress");

        match client.decompress(&wire, true, 1500, Some(payload.len())) {
            Err(Error::Capacity(CapacityError::MessageTooLong { size, max_size })) => {
                assert_eq!(size, max_size + 1, "`already` must be inside the reported size");
                assert!(size > 1500, "the earlier frame's 1500 bytes must be counted");
            }
            other => panic!("`already` must consume the budget, got {other:?}"),
        }
    }

    /// A highly compressible payload against a much smaller budget: the guard has
    /// to fire while inflating rather than after materialising everything.
    #[test]
    fn a_compression_bomb_is_stopped_rather_than_materialised() {
        let payload = vec![0u8; 512 * 1024];
        let (mut server, mut client) = pair();
        let wire = server.compress(&payload).expect("compress");
        assert!(wire.len() < 4096, "the fixture must actually be a bomb: {} bytes", wire.len());

        match client.decompress(&wire, true, 0, Some(8 * 1024)) {
            Err(Error::Capacity(CapacityError::MessageTooLong { size, max_size })) => {
                // `size` is the discriminator, not decoration. `writable` is
                // `remaining + 1`, so detection happens on the first byte past the
                // budget and the reported size is always exactly one over. Using
                // `max_size` instead of `remaining` would let a call produce a
                // whole 4 KiB scratch chunk before the check fires, and the same
                // variant would carry a size thousands of bytes larger. The public
                // error value is where per-call overshoot becomes observable.
                assert_eq!(
                    size,
                    max_size + 1,
                    "detection must happen one byte past the budget, not one chunk past it"
                );
            }
            other => panic!("a bomb against a small budget must be rejected, got {other:?}"),
        }
    }

    /// `progress` is the only thing standing between a stalled codec and an
    /// unbounded loop, so every cell of its table is pinned. Zero progress with
    /// `Ok` is the stall: the backend claims success and consumed and produced
    /// nothing, so the caller would spin forever. `BufError`/`StreamEnd` with
    /// zero progress is ordinary termination.
    ///
    /// Deliberately a pure function rather than a driven loop: a mutant that
    /// removes the guard makes this row *fail*, where a loop-based test would
    /// hang and `cargo test` has no per-test timeout.
    #[test]
    fn zero_progress_on_ok_is_the_only_error_row() {
        for (status, consumed, produced, expected) in [
            (Status::Ok, 0, 0, None),
            (Status::Ok, 1, 0, Some(true)),
            (Status::Ok, 0, 1, Some(true)),
            (Status::BufError, 0, 0, Some(false)),
            (Status::BufError, 1, 0, Some(true)),
            (Status::StreamEnd, 0, 0, Some(false)),
            (Status::StreamEnd, 0, 1, Some(true)),
        ] {
            match (progress(status, consumed, produced), expected) {
                (Ok(got), Some(want)) => {
                    assert_eq!(got, want, "progress({status:?}, {consumed}, {produced})")
                }
                (Err(Error::Protocol(ProtocolError::Compression)), None) => {}
                (got, want) => panic!(
                    "progress({status:?}, {consumed}, {produced}) gave {got:?}, wanted {want:?}"
                ),
            }
        }
    }
}
