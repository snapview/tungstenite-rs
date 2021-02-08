# 0.13.0

- Add `CapacityError`, `UrlError`, and `ProtocolError` types to represent the different types of capacity, URL, and protocol errors respectively.
- Modify variants `Error::Capacity`, `Error::Url`, and `Error::Protocol` to hold the above errors types instead of string error messages.
- Add `handshake::derive_accept_key` to facilitate external handshakes.
- Add support for `rustls` as TLS backend. The previous `tls` feature flag is now removed in favor
  of `native-tls` and `rustls-tls`, which allows to pick the TLS backend. The error API surface had
  to be changed to support the new error types coming from rustls related crates.

# 0.12.0

- Add facilities to allow clients to follow HTTP 3XX redirects.
- Allow accepting unmasked clients on the server side to be compatible with some legacy / invalid clients.
- Update of dependencies and documentation fixes.
