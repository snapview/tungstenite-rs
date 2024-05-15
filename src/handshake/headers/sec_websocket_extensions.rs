use std::convert::TryFrom;

use bytes::BytesMut;
use http::header::SEC_WEBSOCKET_EXTENSIONS;

use util::{Comma, FlatCsv, HeaderValueString, SemiColon};
use {Error, Header, HeaderValue};

/// `Sec-WebSocket-Extensions` header, defined in [RFC6455][RFC6455_11.3.2]
///
/// The `Sec-WebSocket-Extensions` header field is used in the WebSocket
/// opening handshake.  It is initially sent from the client to the
/// server, and then subsequently sent from the server to the client, to
/// agree on a set of protocol-level extensions to use for the duration
/// of the connection.
///
/// ## ABNF
///
/// ```text
/// Sec-WebSocket-Extensions = extension-list
/// extension-list = 1#extension
/// extension = extension-token *( ";" extension-param )
/// extension-token = registered-token
/// registered-token = token
/// extension-param = token [ "=" (token | quoted-string) ]
///     ;When using the quoted-string syntax variant, the value
///     ;after quoted-string unescaping MUST conform to the
///     ;'token' ABNF.
/// ```
///
/// ## Example Values
///
/// * `permessage-deflate` (defined in [RFC7692][RFC7692_7])
/// * `permessage-deflate; server_max_window_bits=10`
/// * `permessage-deflate; server_max_window_bits=10, permessage-deflate`
///
/// ## Example
///
/// ```rust
/// # extern crate headers;
/// use headers::SecWebsocketExtensions;
///
/// let extensions = SecWebsocketExtensions::from_static("permessage-deflate");
/// ```
///
/// ## Splitting and Combining
///
/// Note that `Sec-WebSocket-Extensions` may be split or combined across multiple headers.
/// The following are equivalent:
/// ```text
/// Sec-WebSocket-Extensions: foo
/// Sec-WebSocket-Extensions: bar; baz=2
/// ```
/// ```text
/// Sec-WebSocket-Extensions: foo, bar; baz=2
/// ```
///
/// `SecWebsocketExtensions` splits extensions when decoding and combines them into a single
/// value when encoding.
///
/// [RFC6455_11.3.2]: https://tools.ietf.org/html/rfc6455#section-11.3.2
/// [RFC7692_7]: https://tools.ietf.org/html/rfc7692#section-7
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecWebsocketExtensions(Vec<WebsocketExtension>);

impl Header for SecWebsocketExtensions {
    fn name() -> &'static ::HeaderName {
        &SEC_WEBSOCKET_EXTENSIONS
    }

    fn decode<'i, I: Iterator<Item = &'i HeaderValue>>(values: &mut I) -> Result<Self, Error> {
        let extensions = values
            .cloned()
            .flat_map(|v| {
                FlatCsv::<Comma>::from(v)
                    .iter()
                    .map(WebsocketExtension::try_from)
                    .collect::<Vec<_>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        if extensions.is_empty() {
            Err(Error::invalid())
        } else {
            Ok(SecWebsocketExtensions(extensions))
        }
    }

    fn encode<E: Extend<HeaderValue>>(&self, values: &mut E) {
        if !self.is_empty() {
            values.extend(std::iter::once(self.to_value()));
        }
    }
}

impl SecWebsocketExtensions {
    /// Construct a `SecWebSocketExtensions` from `Vec<WebsocketExtension>`.
    pub fn new(extensions: Vec<WebsocketExtension>) -> Self {
        SecWebsocketExtensions(extensions)
    }

    /// Construct a `SecWebSocketExtensions` from a static string.
    ///
    /// ## Panic
    ///
    /// Panics if the static string is not a valid extensions valie.
    pub fn from_static(s: &'static str) -> Self {
        let value = HeaderValue::from_static(s);
        SecWebsocketExtensions::try_from(&value).expect("valid static string")
    }

    /// Convert this `SecWebsocketExtensions` to a single `HeaderValue`.
    pub fn to_value(&self) -> HeaderValue {
        let values = self.0.iter().map(HeaderValue::from).collect::<FlatCsv>();
        HeaderValue::from(&values)
    }

    /// An iterator over the `WebsocketExtension`s in `SecWebsocketExtensions` header(s).
    pub fn iter(&self) -> impl Iterator<Item = &WebsocketExtension> {
        self.0.iter()
    }

    /// Get the number of extensions.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if headers contain no extensions.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TryFrom<&str> for SecWebsocketExtensions {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = HeaderValue::from_str(value).map_err(|_| Error::invalid())?;
        SecWebsocketExtensions::try_from(&value)
    }
}

impl TryFrom<&HeaderValue> for SecWebsocketExtensions {
    type Error = Error;

    fn try_from(value: &HeaderValue) -> Result<Self, Self::Error> {
        let mut values = std::iter::once(value);
        SecWebsocketExtensions::decode(&mut values)
    }
}

/// A WebSocket extension containing the name and parameters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebsocketExtension {
    name: HeaderValueString,
    params: Vec<(HeaderValueString, Option<HeaderValueString>)>,
}

impl WebsocketExtension {
    /// Construct a `WebSocketExtension` from a static string.
    ///
    /// ## Panics
    ///
    /// This function panics if the argument is invalid.
    pub fn from_static(src: &'static str) -> Self {
        WebsocketExtension::try_from(HeaderValue::from_static(src)).expect("valid static value")
    }

    /// Get the name of the extension.
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// An iterator over the parameters of this extension.
    pub fn params(&self) -> impl Iterator<Item = (&str, Option<&str>)> {
        self.params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_ref().map(|v| v.as_str())))
    }
}

impl TryFrom<&str> for WebsocketExtension {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.is_empty() {
            Err(Error::invalid())
        } else {
            let value = HeaderValue::from_str(value).map_err(|_| Error::invalid())?;
            WebsocketExtension::try_from(value)
        }
    }
}

impl TryFrom<HeaderValue> for WebsocketExtension {
    type Error = Error;

    fn try_from(value: HeaderValue) -> Result<Self, Self::Error> {
        let csv = FlatCsv::<Comma>::from(value);
        // More than one extension was found
        if csv.iter().count() > 1 {
            return Err(Error::invalid());
        }

        let params = FlatCsv::<SemiColon>::from(csv.value);
        let mut params_iter = params.iter();
        let name = params_iter
            .next()
            .ok_or_else(Error::invalid)
            .and_then(parse_token)?;
        let params = params_iter
            .map(|p| {
                let mut kv = p.splitn(2, '=');
                let key = kv
                    .next()
                    .ok_or_else(Error::invalid)
                    .map(str::trim)
                    .and_then(parse_token)?;
                let val = kv.next().map(str::trim).map(parse_value).transpose()?;
                Ok((key, val))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WebsocketExtension { name, params })
    }
}

impl From<&WebsocketExtension> for HeaderValue {
    fn from(extension: &WebsocketExtension) -> Self {
        let mut buf = BytesMut::from(extension.name.as_str().as_bytes());
        for (key, val) in &extension.params {
            buf.extend_from_slice(b"; ");
            buf.extend_from_slice(key.as_str().as_bytes());
            if let Some(val) = val {
                buf.extend_from_slice(b"=");
                buf.extend_from_slice(val.as_str().as_bytes());
            }
        }

        HeaderValue::from_maybe_shared(buf.freeze())
            .expect("semicolon separated HeaderValueStrings are valid")
    }
}

fn parse_token(s: &str) -> Result<HeaderValueString, Error> {
    if !s.is_empty() && s.chars().all(is_tchar) {
        HeaderValueString::from_str(s)
    } else {
        Err(Error::invalid())
    }
}

// https://datatracker.ietf.org/doc/html/rfc7230#section-3.2.6
fn is_tchar(c: char) -> bool {
    matches!(
        c,
        '!' | '#' | '$' | '%' | '&' | '\'' | '*' |
        '+' | '-' | '.' | '^' | '_' | '`' | '|' | '~' |
        '0'..='9' | 'a'..='z' | 'A'..='Z'
    )
}

fn parse_value(s: &str) -> Result<HeaderValueString, Error> {
    if let Some(quoted) = s.strip_prefix('"') {
        if let Some(val) = quoted.strip_suffix('"') {
            parse_token(val)
        } else {
            // Only had starting double quote
            Err(Error::invalid())
        }
    } else {
        // Not a quoted string.
        parse_token(s)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{test_decode, test_encode};
    use super::*;

    #[test]
    fn extensions_decode() {
        let extensions =
            test_decode::<SecWebsocketExtensions>(&["key1; val1", "key2; val2"]).unwrap();
        assert_eq!(extensions.0.len(), 2);
        assert_eq!(
            extensions.0[0],
            WebsocketExtension::try_from("key1; val1").unwrap()
        );
        assert_eq!(
            extensions.0[1],
            WebsocketExtension::try_from("key2; val2").unwrap()
        );

        assert_eq!(test_decode::<SecWebsocketExtensions>(&[""]), None);
    }

    #[test]
    fn extensions_decode_split() {
        // Split each extension into separate headers
        let extensions =
            test_decode::<SecWebsocketExtensions>(&["key1; val1, key2; val2", "key3; val3"])
                .unwrap();
        assert_eq!(extensions.0.len(), 3);
        assert_eq!(
            extensions.0[0],
            WebsocketExtension::try_from("key1; val1").unwrap()
        );
        assert_eq!(
            extensions.0[1],
            WebsocketExtension::try_from("key2; val2").unwrap()
        );
        assert_eq!(
            extensions.0[2],
            WebsocketExtension::try_from("key3; val3").unwrap()
        );
    }

    #[test]
    fn extensions_encode() {
        let extensions =
            SecWebsocketExtensions::new(vec![WebsocketExtension::from_static("foo; bar; baz=1")]);
        let headers = test_encode(extensions);
        let mut vals = headers.get_all(SEC_WEBSOCKET_EXTENSIONS).into_iter();
        assert_eq!(vals.next().unwrap(), "foo; bar; baz=1");
        assert_eq!(vals.next(), None);

        let extensions = SecWebsocketExtensions::new(vec![]);
        let headers = test_encode(extensions);
        let mut vals = headers.get_all(SEC_WEBSOCKET_EXTENSIONS).into_iter();
        assert_eq!(vals.next(), None);
    }

    #[test]
    fn extensions_encode_combine() {
        // Multiple extensions are combined into a single header
        let extensions = SecWebsocketExtensions::new(vec![
            WebsocketExtension::from_static("foo1; bar"),
            WebsocketExtension::from_static("foo2; bar"),
            WebsocketExtension::from_static("baz; quux"),
        ]);
        let headers = test_encode(extensions);
        let mut vals = headers.get_all(SEC_WEBSOCKET_EXTENSIONS).into_iter();
        assert_eq!(vals.next().unwrap(), "foo1; bar, foo2; bar, baz; quux");
        assert_eq!(vals.next(), None);
    }

    #[test]
    fn extensions_iter() {
        let extensions = SecWebsocketExtensions::new(vec![
            WebsocketExtension::from_static("foo; bar1; bar2=3"),
            WebsocketExtension::from_static("baz; quux"),
        ]);
        assert_eq!(extensions.len(), 2);

        let mut iter = extensions.iter();
        let extension = iter.next().unwrap();
        assert_eq!(extension.name(), "foo");
        let mut params = extension.params();
        assert_eq!(params.next(), Some(("bar1", None)));
        assert_eq!(params.next(), Some(("bar2", Some("3"))));
        assert!(params.next().is_none());

        let extension = iter.next().unwrap();
        assert_eq!(extension.name(), "baz");
        let mut params = extension.params();
        assert_eq!(params.next(), Some(("quux", None)));
        assert!(params.next().is_none());

        assert!(iter.next().is_none());
    }

    #[test]
    fn extension_try_from_str_ok() {
        let ext = WebsocketExtension::try_from("permessage-deflate").unwrap();
        assert_eq!(ext.name(), "permessage-deflate");
        let mut params = ext.params();
        assert_eq!(params.next(), None);

        let ext =
            WebsocketExtension::try_from("permessage-deflate; client_max_window_bits").unwrap();
        assert_eq!(ext.name(), "permessage-deflate");
        let mut params = ext.params();
        assert_eq!(params.next(), Some(("client_max_window_bits", None)));
        assert_eq!(params.next(), None);

        let ext =
            WebsocketExtension::try_from("permessage-deflate; server_max_window_bits=10").unwrap();
        assert_eq!(ext.name(), "permessage-deflate");
        let mut params = ext.params();
        assert_eq!(params.next(), Some(("server_max_window_bits", Some("10"))));
        assert_eq!(params.next(), None);

        let ext = WebsocketExtension::try_from("permessage-deflate; server_max_window_bits=\"10\"")
            .unwrap();
        assert_eq!(ext.name(), "permessage-deflate");
        let mut params = ext.params();
        assert_eq!(params.next(), Some(("server_max_window_bits", Some("10"))));
        assert_eq!(params.next(), None);
    }

    #[test]
    fn extension_try_from_str_err() {
        assert!(WebsocketExtension::try_from("").is_err());
        // Only single extension is allowed
        assert!(WebsocketExtension::try_from("permessage-deflate, permessage-snappy").is_err());
    }

    #[test]
    fn parse_value_err() {
        #[rustfmt::skip]
        let cases = [
            // not token
            "",
            " ",
            // Only starting quote
            r#"""#,
            r#""10"#,
            // Multiple quotes
            r#"""1"""#,
            // Not a token after removing quotes
            r#"" ""#,
            r#"",""#,
        ];
        for case in cases {
            assert!(parse_value(case).is_err());
        }
    }

    #[test]
    fn parse_value_ok() {
        #[rustfmt::skip]
        let cases = [
            // Not quoted
            r#"1"#,
            r#"10"#,
            r#"10.1"#,
            // valid quoted-string
            r#""9""#,
            r#""val""#,
        ];
        for case in cases {
            assert!(parse_value(case).is_ok());
        }
    }
}
