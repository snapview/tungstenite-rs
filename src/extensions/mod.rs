//! WebSocket extensions.
// Only `permessage-deflate` is supported at the moment.

mod compression;
pub use compression::deflate::{DeflateConfig, DeflateContext, DeflateError};
use http::HeaderValue;

/// Iterator of all extension offers/responses in `Sec-WebSocket-Extensions` values.
pub(crate) fn iter_all<'a>(
    values: impl Iterator<Item = &'a HeaderValue>,
) -> impl Iterator<Item = (&'a str, impl Iterator<Item = (&'a str, Option<&'a str>)>)> {
    values
        .filter_map(|h| h.to_str().ok())
        .map(|value_str| {
            split_iter(value_str, ',').filter_map(|offer| {
                // Parameters are separted by semicolons.
                // The first element is the name of the extension.
                let mut iter = split_iter(offer.trim(), ';').map(str::trim);
                let name = iter.next()?;
                let params = iter.filter_map(|kv| {
                    let mut it = kv.splitn(2, '=');
                    let key = it.next()?.trim();
                    let val = it.next().map(|v| v.trim().trim_matches('"'));
                    Some((key, val))
                });
                Some((name, params))
            })
        })
        .flatten()
}

fn split_iter(input: &str, sep: char) -> impl Iterator<Item = &str> {
    let mut in_quotes = false;
    let mut prev = None;
    input.split(move |c| {
        if in_quotes {
            if c == '"' && prev != Some('\\') {
                in_quotes = false;
            }
            prev = Some(c);
            false
        } else if c == sep {
            prev = Some(c);
            true
        } else {
            if c == '"' {
                in_quotes = true;
            }
            prev = Some(c);
            false
        }
    })
}

#[cfg(test)]
mod tests {
    use http::{header::SEC_WEBSOCKET_EXTENSIONS, HeaderMap};

    use super::*;

    // Make sure comma separated offers and multiple headers are equivalent
    fn test_iteration<'a>(
        mut iter: impl Iterator<Item = (&'a str, impl Iterator<Item = (&'a str, Option<&'a str>)>)>,
    ) {
        let (name, mut params) = iter.next().unwrap();
        assert_eq!(name, "permessage-deflate");
        assert_eq!(params.next(), Some(("client_max_window_bits", None)));
        assert_eq!(params.next(), Some(("server_max_window_bits", Some("10"))));
        assert!(params.next().is_none());

        let (name, mut params) = iter.next().unwrap();
        assert_eq!(name, "permessage-deflate");
        assert_eq!(params.next(), Some(("client_max_window_bits", None)));
        assert!(params.next().is_none());

        assert!(iter.next().is_none());
    }

    #[test]
    fn iter_single() {
        let mut hm = HeaderMap::new();
        hm.append(
            SEC_WEBSOCKET_EXTENSIONS,
            HeaderValue::from_static(
                "permessage-deflate; client_max_window_bits; server_max_window_bits=10, permessage-deflate; client_max_window_bits",
            ),
        );
        test_iteration(iter_all(std::iter::once(hm.get(SEC_WEBSOCKET_EXTENSIONS).unwrap())));
    }

    #[test]
    fn iter_multiple() {
        let mut hm = HeaderMap::new();
        hm.append(
            SEC_WEBSOCKET_EXTENSIONS,
            HeaderValue::from_static(
                "permessage-deflate; client_max_window_bits; server_max_window_bits=10",
            ),
        );
        hm.append(
            SEC_WEBSOCKET_EXTENSIONS,
            HeaderValue::from_static("permessage-deflate; client_max_window_bits"),
        );
        test_iteration(iter_all(hm.get_all(SEC_WEBSOCKET_EXTENSIONS).iter()));
    }
}

// TODO More strict parsing
// https://datatracker.ietf.org/doc/html/rfc6455#section-4.3
// Sec-WebSocket-Extensions = extension-list
// extension-list = 1#extension
// extension = extension-token *( ";" extension-param )
// extension-token = registered-token
// registered-token = token
// extension-param = token [ "=" (token | quoted-string) ]
//     ;When using the quoted-string syntax variant, the value
//     ;after quoted-string unescaping MUST conform to the
//     ;'token' ABNF.
//
// token          = 1*<any CHAR except CTLs or separators>
// CHAR           = <any US-ASCII character (octets 0 - 127)>
// CTL            = <any US-ASCII control character (octets 0 - 31) and DEL (127)>
// separators     = "(" | ")" | "<" | ">" | "@"
//                   | "," | ";" | ":" | "\" | <">
//                   | "/" | "[" | "]" | "?" | "="
//                   | "{" | "}" | SP | HT
// SP             = <US-ASCII SP, space (32)>
// HT             = <US-ASCII HT, horizontal-tab (9)>
// quoted-string  = ( <"> *(qdtext | quoted-pair ) <"> )
// qdtext         = <any TEXT except <">>
// quoted-pair    = "\" CHAR
