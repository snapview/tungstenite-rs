# 0.15.0 (unreleased)

- Allow selecting the method of loading root certificates if `rustls` is used as TLS implementation.
  - Two new feature flags `rustls-tls-native-roots` and `rustls-tls-webpki-roots` have been added
    that activate the respective method to load certificates.
  - The `rustls-tls` flag was removed to raise awareness of this change. Otherwise, compilation
    would have continue to work and potential errors (due to different or missing certificates)
    only occurred at runtime.
  - The new feature flags are additive. If both are enabled, both methods will be used to add
    certificates to the TLS configuration.

# 0.14.0

- Use `rustls-native-certs` instead of `webpki-root` when `rustls-tls` feature is enabled.
- Don't use `native-tls` as a default feature (see #202 for more details).
- New fast and safe implementation of the reading buffer (replacement for the `input_buffer`).
- Remove some errors from the `Error` enum that can't be triggered anymore with the new buffer implementation.

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
