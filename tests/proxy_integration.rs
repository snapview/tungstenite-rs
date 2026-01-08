#![cfg(feature = "proxy")]

use std::{
    env,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

use tungstenite::{accept, connect, Message};

fn run_proxy_test(proxy_env_key: &str, proxy_scheme: &str) {
    let (target_port, target_handle) = spawn_ws_echo_server();
    let target_addr = format!("127.0.0.1:{target_port}");

    let (proxy_port, proxy_handle) = spawn_proxy(proxy_env_key, &target_addr);

    let prev_http_proxy = env::var("HTTP_PROXY").ok();
    let prev_https_proxy = env::var("HTTPS_PROXY").ok();
    let prev_all_proxy = env::var("ALL_PROXY").ok();
    let prev_no_proxy = env::var("NO_PROXY").ok();

    env::remove_var("HTTP_PROXY");
    env::remove_var("HTTPS_PROXY");
    env::remove_var("ALL_PROXY");
    env::remove_var("NO_PROXY");

    let proxy_url = format!("{proxy_scheme}://127.0.0.1:{proxy_port}");
    env::set_var(proxy_env_key, proxy_url);

    let url = format!("ws://{target_addr}");
    let (mut socket, _response) = connect(url).expect("proxy connect");
    socket.send(Message::Text("hello".into())).expect("send");
    let msg = socket.read().expect("read");
    assert_eq!(msg, Message::Text("hello".into()));
    let _ = socket.close(None);

    restore_env("HTTP_PROXY", prev_http_proxy);
    restore_env("HTTPS_PROXY", prev_https_proxy);
    restore_env("ALL_PROXY", prev_all_proxy);
    restore_env("NO_PROXY", prev_no_proxy);

    proxy_handle.join().expect("proxy thread");
    target_handle.join().expect("target thread");
}

fn restore_env(key: &str, value: Option<String>) {
    match value {
        Some(value) => env::set_var(key, value),
        None => env::remove_var(key),
    }
}

fn spawn_ws_echo_server() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ws server");
    let port = listener.local_addr().expect("addr").port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut ws = accept(stream).expect("accept ws");
        if let Ok(msg) = ws.read() {
            let _ = ws.send(msg);
        }
        let _ = ws.close(None);
    });
    (port, handle)
}

fn spawn_proxy(proxy_env_key: &str, target_addr: &str) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy");
    let port = listener.local_addr().expect("addr").port();
    let target_addr = target_addr.to_string();
    let proxy_env_key = proxy_env_key.to_string();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        if proxy_env_key == "HTTP_PROXY" {
            handle_http_connect(stream, &target_addr);
        } else {
            handle_socks5(stream, &target_addr);
        }
    });
    (port, handle)
}

fn handle_http_connect(mut client: TcpStream, target_addr: &str) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        let read = client.read(&mut chunk).expect("read");
        if read == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }

    if !std::str::from_utf8(&buf).unwrap_or("").starts_with("CONNECT") {
        return;
    }

    let mut upstream = TcpStream::connect(target_addr).expect("connect upstream");
    set_timeouts(&client);
    set_timeouts(&upstream);
    client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").expect("write");

    let mut client_clone = client.try_clone().expect("clone client");
    let mut upstream_clone = upstream.try_clone().expect("clone upstream");
    let t1 = thread::spawn(move || {
        let _ = std::io::copy(&mut client_clone, &mut upstream_clone);
    });
    let _ = std::io::copy(&mut upstream, &mut client);
    let _ = t1.join();
}

fn handle_socks5(mut client: TcpStream, target_addr: &str) {
    let mut header = [0u8; 2];
    client.read_exact(&mut header).expect("read greeting");
    let methods_len = header[1] as usize;
    let mut methods = vec![0u8; methods_len];
    client.read_exact(&mut methods).expect("read methods");
    client.write_all(&[0x05, 0x00]).expect("write method");

    let mut req = [0u8; 4];
    client.read_exact(&mut req).expect("read request");
    if req[1] != 0x01 {
        return;
    }

    let addr = match req[3] {
        0x01 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip).expect("read ip");
            std::net::Ipv4Addr::from(ip).to_string()
        }
        0x03 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).expect("read len");
            let mut host = vec![0u8; len[0] as usize];
            client.read_exact(&mut host).expect("read host");
            String::from_utf8_lossy(&host).to_string()
        }
        0x04 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip).expect("read ip");
            std::net::Ipv6Addr::from(ip).to_string()
        }
        _ => return,
    };

    let mut port = [0u8; 2];
    client.read_exact(&mut port).expect("read port");
    let port = u16::from_be_bytes(port);
    let _ = (addr, port);

    let mut upstream = TcpStream::connect(target_addr).expect("connect upstream");
    set_timeouts(&client);
    set_timeouts(&upstream);
    client
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .expect("write reply");

    let mut client_clone = client.try_clone().expect("clone client");
    let mut upstream_clone = upstream.try_clone().expect("clone upstream");
    let t1 = thread::spawn(move || {
        let _ = std::io::copy(&mut client_clone, &mut upstream_clone);
    });
    let _ = std::io::copy(&mut upstream, &mut client);
    let _ = t1.join();
}

fn set_timeouts(stream: &TcpStream) {
    let timeout = Duration::from_secs(2);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
}

#[test]
fn proxy_http_and_socks5() {
    run_proxy_test("HTTP_PROXY", "http");
    run_proxy_test("ALL_PROXY", "socks5");
}
