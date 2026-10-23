use std::borrow::Cow;

use http::{header::SEC_WEBSOCKET_EXTENSIONS, HeaderMap, HeaderValue};

use super::{Settings, NAME};
use crate::error::{Error, ProtocolError, Result};

pub(super) fn offer(settings: Settings) -> HeaderValue {
    let mut value = String::from(NAME);
    if settings.server_no_context_takeover {
        value.push_str("; server_no_context_takeover");
    }
    if settings.client_no_context_takeover {
        value.push_str("; client_no_context_takeover");
    }
    if settings.server_max_window_bits < 15 {
        value.push_str(&format!("; server_max_window_bits={}", settings.server_max_window_bits));
    }
    // Valueless rather than the configured cap: that cap bounds our own encoder and is
    // applied to whatever the server selects, so naming it here would put a promise on the
    // wire without changing the window we compress with. RFC 7692 §7.1.2.2 gives the bare
    // form exactly that meaning -- support for a selection, with no hint attached.
    value.push_str("; client_max_window_bits");
    HeaderValue::from_str(&value).expect("the generated extension offer is valid")
}

pub(super) fn accept_response(settings: Settings, headers: &HeaderMap) -> Result<Option<Settings>> {
    let mut selected = None;
    for value in headers.get_all(SEC_WEBSOCKET_EXTENSIONS) {
        for extension in split_header(value.as_bytes(), b',')? {
            match parse(extension)? {
                Extension::Empty => {}
                Extension::Deflate(params) if selected.is_none() => selected = Some(params),
                // permessage-deflate is the only extension token the offer names, so another
                // name here was never requested and a second instance has no second codec.
                Extension::Deflate(_) | Extension::Other => return Err(invalid_header()),
            }
        }
    }
    let Some(params) = selected else {
        return Ok(None);
    };
    if settings.server_no_context_takeover && !params.server_no_context_takeover {
        return Err(invalid_header());
    }
    let mut agreed = settings;
    agreed.server_no_context_takeover = params.server_no_context_takeover;
    agreed.client_no_context_takeover |= params.client_no_context_takeover;
    match params.server_max_window_bits {
        Some(bits) if bits > settings.server_max_window_bits => return Err(invalid_header()),
        Some(bits) => agreed.server_max_window_bits = bits,
        None if settings.server_max_window_bits < 15 => return Err(invalid_header()),
        None => {}
    }
    // The offer invited a selection, so RFC 7692 §7.1.2.2 governs every shape of answer: a
    // value limits the window we compress with, and absence means the server can decode a
    // full 32 KiB, which the configured cap is never above.
    match params.client_max_window_bits {
        // flate2 builds compressors only for 9..=15, and clamping up to 9 would encode
        // wider than the server just agreed to decode.
        ClientWindow::Bits(8) => return Err(invalid_header()),
        ClientWindow::Bits(bits) => {
            agreed.client_max_window_bits = bits.min(settings.client_max_window_bits);
        }
        // Only an offer may go bare; a response must carry a decimal value.
        ClientWindow::NoValue => return Err(invalid_header()),
        ClientWindow::Absent => {}
    }
    Ok(Some(agreed))
}

pub(super) fn accept_offers(
    settings: Settings,
    offers: &[HeaderValue],
) -> Option<(Settings, HeaderValue)> {
    // Only an offer naming client_max_window_bits lets the response bind the peer, while
    // `accept_offer` gives every other accepted offer 15. Under a reduced cap, an agreed window
    // below 15 is exactly the binding case and outranks an earlier full-window alternative.
    // RFC 7692 §5 only lowercase-recommends order; §7.1.3 lets the server pick any it supports.
    let mut fallback = None;
    for value in offers {
        for extension in split_header(value.as_bytes(), b',').into_iter().flatten() {
            if let Ok(Extension::Deflate(offer)) = parse(extension) {
                let Some(accepted) = accept_offer(settings, offer) else { continue };
                if settings.client_max_window_bits == 15 || accepted.0.client_max_window_bits < 15 {
                    return Some(accepted);
                }
                fallback.get_or_insert(accepted);
            }
        }
    }
    fallback
}

fn accept_offer(settings: Settings, offer: Params) -> Option<(Settings, HeaderValue)> {
    let mut agreed = settings;
    agreed.server_no_context_takeover |= offer.server_no_context_takeover;
    agreed.client_no_context_takeover |= offer.client_no_context_takeover;

    let server_window = match offer.server_max_window_bits {
        // This is our own encoder's window, and an offered 8 would reach `Compress`, which
        // asserts 9..=15. Refuse the offer; clamping to 9 would compress with a wider
        // window than the client asked for. `accept_response` refuses a selected 8 for the
        // same reason, on the window this endpoint would then encode with.
        Some(8) => return None,
        Some(bits) => Some(bits.min(settings.server_max_window_bits)),
        None if settings.server_max_window_bits < 15 => Some(settings.server_max_window_bits),
        None => None,
    };
    if let Some(bits) = server_window {
        agreed.server_max_window_bits = bits;
    }

    let client_window = match offer.client_max_window_bits {
        // RFC 7692 §7.2.2: with no agreed client_max_window_bits the server must use a 32 KiB
        // window, so a configured cap cannot bind a client that never offered one. Installing
        // 15 keeps the decoder large enough for output the peer may produce; the response still
        // omits the parameter, which §7.1.2.2 requires of an offer that did not name it.
        ClientWindow::Absent => Some(15),
        ClientWindow::NoValue => Some(settings.client_max_window_bits),
        ClientWindow::Bits(bits) => Some(bits.min(settings.client_max_window_bits)),
    };
    if let Some(bits) = client_window {
        agreed.client_max_window_bits = bits;
    }

    let mut response = String::from(NAME);
    if agreed.server_no_context_takeover {
        response.push_str("; server_no_context_takeover");
    }
    if agreed.client_no_context_takeover {
        response.push_str("; client_no_context_takeover");
    }
    if let Some(bits) = server_window {
        response.push_str(&format!("; server_max_window_bits={bits}"));
    }
    if let Some(bits) = client_window.filter(|bits| *bits < 15) {
        response.push_str(&format!("; client_max_window_bits={bits}"));
    }
    Some((agreed, HeaderValue::from_str(&response).expect("generated response is valid")))
}

#[derive(Default)]
struct Params {
    server_no_context_takeover: bool,
    client_no_context_takeover: bool,
    server_max_window_bits: Option<u8>,
    client_max_window_bits: ClientWindow,
    seen: u8,
}

#[derive(Default)]
enum ClientWindow {
    #[default]
    Absent,
    NoValue,
    Bits(u8),
}

/// One element of a `Sec-WebSocket-Extensions` list.
///
/// A server skips an extension it was not asked about; a client must fail on one it
/// never offered. Keeping the two apart is the caller's decision, not the parser's.
enum Extension {
    Empty,
    Deflate(Params),
    Other,
}

fn parse(extension: &[u8]) -> Result<Extension> {
    let name = trim_ascii(extension.split(|byte| *byte == b';').next().unwrap_or_default());
    if name.is_empty() {
        return if trim_ascii(extension).is_empty() {
            Ok(Extension::Empty)
        } else {
            Err(invalid_header())
        };
    }
    // Classify the ASCII extension token before decoding its parameters so an
    // unrelated extension may carry any valid HeaderValue bytes.
    if !name.eq_ignore_ascii_case(NAME.as_bytes()) {
        return Ok(Extension::Other);
    }
    let extension = std::str::from_utf8(extension).map_err(|_| invalid_header())?;
    let parts = split_quoted(extension, b';')?;
    // HTTP field values use case-insensitive tokens; accept that leniency for
    // extension and parameter names while keeping parameter values exact.
    let mut parsed = Params::default();
    for parameter in &parts[1..] {
        let (name, value) = parameter
            .trim()
            .split_once('=')
            .map_or((parameter.trim(), None), |(name, value)| (name.trim(), Some(value.trim())));
        let bit = if name.eq_ignore_ascii_case("server_no_context_takeover") {
            1
        } else if name.eq_ignore_ascii_case("client_no_context_takeover") {
            2
        } else if name.eq_ignore_ascii_case("server_max_window_bits") {
            4
        } else if name.eq_ignore_ascii_case("client_max_window_bits") {
            8
        } else {
            return Err(invalid_header());
        };
        if parsed.seen & bit != 0 {
            return Err(invalid_header());
        }
        parsed.seen |= bit;
        match bit {
            1 if value.is_none() => parsed.server_no_context_takeover = true,
            2 if value.is_none() => parsed.client_no_context_takeover = true,
            4 => {
                parsed.server_max_window_bits = Some(parse_bits(value.ok_or_else(invalid_header)?)?)
            }
            8 => {
                parsed.client_max_window_bits = match value {
                    Some(value) => ClientWindow::Bits(parse_bits(value)?),
                    None => ClientWindow::NoValue,
                }
            }
            _ => return Err(invalid_header()),
        }
    }
    Ok(Extension::Deflate(parsed))
}

fn parse_bits(value: &str) -> Result<u8> {
    let value = unquote(value)?;
    if !value.bytes().all(|byte| byte.is_ascii_digit()) || value.len() > 1 && value.starts_with('0')
    {
        return Err(invalid_header());
    }
    value.parse::<u8>().ok().filter(|bits| (8..=15).contains(bits)).ok_or_else(invalid_header)
}

/// Split a UTF-8 field value on an unquoted ASCII delimiter.
///
/// Every split point is an ASCII byte outside a quoted string, so it is a character
/// boundary and each part is still valid UTF-8.
fn split_quoted(value: &str, delimiter: u8) -> Result<Vec<&str>> {
    split_header(value.as_bytes(), delimiter)?
        .into_iter()
        .map(|part| std::str::from_utf8(part).map_err(|_| invalid_header()))
        .collect()
}

fn unquote(value: &str) -> Result<Cow<'_, str>> {
    if !value.starts_with('"') {
        return if value.contains('"') { Err(invalid_header()) } else { Ok(Cow::Borrowed(value)) };
    }
    let mut output = String::new();
    let mut chars = value[1..].chars();
    while let Some(character) = chars.next() {
        match character {
            '\\' => output.push(chars.next().ok_or_else(invalid_header)?),
            '"' if chars.next().is_none() => return Ok(Cow::Owned(output)),
            '"' => return Err(invalid_header()),
            character => output.push(character),
        }
    }
    Err(invalid_header())
}

fn invalid_header() -> Error {
    ProtocolError::InvalidHeader(SEC_WEBSOCKET_EXTENSIONS.clone().into()).into()
}

fn split_header(value: &[u8], delimiter: u8) -> Result<Vec<&[u8]>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in value.iter().copied().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == delimiter {
            parts.push(&value[start..index]);
            start = index + 1;
        }
    }
    if quoted || escaped {
        return Err(invalid_header());
    }
    parts.push(&value[start..]);
    Ok(parts)
}

/// Does a peer's `Sec-WebSocket-Extensions` field select permessage-deflate?
///
/// Only for a selection this endpoint did not make. Just the extension token is read:
/// an unrelated extension belongs to whoever negotiated it, and its parameters are not
/// ours to validate. A list we cannot split is an error, never an absence.
pub(crate) fn headers_select_deflate(headers: &HeaderMap) -> Result<bool> {
    for value in headers.get_all(SEC_WEBSOCKET_EXTENSIONS) {
        for extension in split_header(value.as_bytes(), b',')? {
            if extension_is_deflate(extension) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn extension_is_deflate(extension: &[u8]) -> bool {
    let name = extension.split(|byte| *byte == b';').next().unwrap_or_default();
    trim_ascii(name).eq_ignore_ascii_case(NAME.as_bytes())
}

fn trim_ascii(value: &[u8]) -> &[u8] {
    let start = value.iter().position(|byte| !byte.is_ascii_whitespace()).unwrap_or(value.len());
    let end = value.iter().rposition(|byte| !byte.is_ascii_whitespace()).map_or(start, |i| i + 1);
    &value[start..end]
}
