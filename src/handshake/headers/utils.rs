use std::iter::{once, FromIterator};

use bytes::{BufMut, BytesMut};
use http::HeaderValue;

#[derive(Debug, Clone)]
pub(crate) struct FlatCsv<const SEP: char = ','> {
    pub(crate) value: HeaderValue,
}

impl<const SEP: char> FlatCsv<SEP> {
    const SEP_BYTES: [u8; 2] = [SEP as u8, b' '];

    pub(crate) fn iter(&self) -> impl Iterator<Item = &str> {
        FlatCsvIterator::<SEP>(self.value.to_str().ok()).map(str::trim)
    }
}

struct FlatCsvIterator<'a, const SEP: char>(Option<&'a str>);

impl<'a, const SEP: char> Iterator for FlatCsvIterator<'a, SEP> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let str = self.0?;
        let mut in_quotes = false;

        for (idx, chr) in str.char_indices() {
            if chr == '"' {
                in_quotes = !in_quotes;
            }

            if !in_quotes && chr == SEP {
                self.0 = Some(&str[idx + SEP.len_utf8()..]);
                return Some(&str[..idx]);
            }
        }

        self.0 = None;
        Some(str)
    }
}

impl<const SEP: char> FromIterator<HeaderValue> for FlatCsv<SEP> {
    fn from_iter<T: IntoIterator<Item = HeaderValue>>(iter: T) -> Self {
        let mut iter = iter.into_iter();

        let first = match iter.next() {
            None => return HeaderValue::from_static("").into(),
            Some(first) => first,
        };

        let second = match iter.next() {
            None => return first.into(),
            Some(second) => second,
        };

        let mut buf = BytesMut::from(first.as_bytes());

        for value in once(second).chain(iter) {
            buf.put(Self::SEP_BYTES.as_ref());
            buf.put(value.as_bytes());
        }

        HeaderValue::from_maybe_shared(buf.freeze())
            .expect("delimited valid header values to be a valid header value")
            .into()
    }
}

impl<const SEP: char> From<HeaderValue> for FlatCsv<SEP> {
    fn from(value: HeaderValue) -> Self {
        Self { value }
    }
}

impl<const SEP: char> From<FlatCsv<SEP>> for HeaderValue {
    fn from(value: FlatCsv<SEP>) -> Self {
        value.value
    }
}
