//! WebSocket extensions.
// Only `permessage-deflate` is supported at the moment.

use std::borrow::Cow;

mod compression;
pub use compression::deflate::{DeflateConfig, DeflateContext, DeflateError};

/// Extension parameter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Param<'a> {
    name: Cow<'a, str>,
    value: Option<Cow<'a, str>>,
}

impl<'a> Param<'a> {
    /// Create a new parameter with name.
    pub fn new(name: impl Into<Cow<'a, str>>) -> Self {
        Param { name: name.into(), value: None }
    }

    /// Consume itself to create a parameter with value.
    pub fn with_value(mut self, value: impl Into<Cow<'a, str>>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Get the name of the parameter.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the optional value of the parameter.
    pub fn value(&self) -> Option<&str> {
        self.value.as_ref().map(|v| v.as_ref())
    }
}

// NOTE This doesn't support quoted values
/// Parse `Sec-WebSocket-Extensions` offer/response.
pub(crate) fn parse_header(exts: &str) -> Vec<(Cow<'_, str>, Vec<Param<'_>>)> {
    let mut collected = Vec::new();
    // ext-name; a; b=c, ext-name; x, y=z
    for ext in exts.split(',') {
        let mut parts = ext.split(';');
        if let Some(name) = parts.next().map(str::trim) {
            let mut params = Vec::new();
            for p in parts {
                let mut kv = p.splitn(2, '=');
                if let Some(key) = kv.next().map(str::trim) {
                    let param = if let Some(value) = kv.next().map(str::trim) {
                        Param::new(key).with_value(value)
                    } else {
                        Param::new(key)
                    };
                    params.push(param);
                }
            }
            collected.push((Cow::from(name), params));
        }
    }
    collected
}

#[test]
fn test_parse_extensions() {
    let extensions = "permessage-deflate; client_max_window_bits; server_max_window_bits=10, permessage-deflate; client_max_window_bits";
    assert_eq!(
        parse_header(extensions),
        vec![
            (
                Cow::from("permessage-deflate"),
                vec![
                    Param::new("client_max_window_bits"),
                    Param::new("server_max_window_bits").with_value("10")
                ]
            ),
            (Cow::from("permessage-deflate"), vec![Param::new("client_max_window_bits")])
        ]
    );
}
