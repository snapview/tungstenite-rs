# Tungstenite

Lightweight stream-based WebSocket implementation for [Rust](http://www.rust-lang.org).

```rust

/// A WebSocket echo server
let server = TcpListener::bind("127.0.0.1:9001").unwrap();
for stream in server.incoming() {
    spawn (move || {
        let mut websocket = accept(stream.unwrap());
        loop {
            let msg = websocket.read_message().unwrap();
            websocket.write_message(msg).unwrap();
        }
    })
}
```

Introduction
------------
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

This library provides an implementation of WebSockets,
[RFC6455](https://tools.ietf.org/html/rfc6455). It allows for both synchronous (like TcpStream)
and asynchronous usage and is easy to integrate into any third-party event loops including
[MIO](https://github.com/carllerche/mio). The API design abstracts away all the internals of the
WebSocket protocol but still makes them accessible for those who wants full control over the
network.

This library is a work in progress. Feel free to ask questions and send us pull requests.

Why Tungstenite?
----------------

It's formerly WS2, the 2nd implementation of WS. WS2 is the chemical formula of
tungsten disulfide, the tungstenite mineral.

Features
--------

Tungstenite provides a complete implementation of the WebSocket specification.
TLS is supported on all platforms using native-tls.

There is no support for permessage-deflate at the moment. It's planned.

Testing
-------

Tungstenite is thoroughly tested and passes the [Autobahn Test Suite](http://autobahn.ws/testsuite/) for
WebSockets. It is also covered by internal unit tests as good as possible.

Contributing
------------

Please report bugs and make feature requests [here](https://github.com/snapview/tungstenite-rs/issues).
