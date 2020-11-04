use crate::extensions::compression::deflate::{
    on_receive_request, DeflateConfigBuilder, DeflateExtensionError,
};
use http::header::SEC_WEBSOCKET_EXTENSIONS;
use http::{HeaderValue, Request, Response};

mod server {
    use super::*;

    #[test]
    fn config_unchanged_on_err() {
        let s = "permessage-deflate; client_no_context_takeover; client_max_window_bits; server_no_context_takeover; server_max_window_bits=\"80000\"";
        let mut request = Request::new(());
        request
            .headers_mut()
            .insert(SEC_WEBSOCKET_EXTENSIONS, HeaderValue::from_static(s));

        let mut response = Response::new(());
        let initial_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(11)
            .build();

        let mut parsed_config = initial_config.clone();

        let r = on_receive_request(&request, &mut response, &mut parsed_config);

        assert_eq!(r, Err(DeflateExtensionError::InvalidMaxWindowBits));
        assert_eq!(initial_config, parsed_config);
    }

    #[test]
    fn missing_client_window_size() {
        let s = "permessage-deflate; client_max_window_bits";
        let mut request = Request::new(());
        request
            .headers_mut()
            .insert(SEC_WEBSOCKET_EXTENSIONS, HeaderValue::from_static(s));

        let mut response = Response::new(());
        let mut initial_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(11)
            .build();

        let r = on_receive_request(&request, &mut response, &mut initial_config);

        assert!(r.is_ok());
        let parsed_header = response
            .headers()
            .get(SEC_WEBSOCKET_EXTENSIONS)
            .expect("Missing header")
            .to_str()
            .expect("Failed to parse header");

        assert_eq!(
            parsed_header,
            "permessage-deflate; client_max_window_bits=11; server_max_window_bits=10"
        );
    }

    #[test]
    fn missing_server_window_size() {
        let s = "permessage-deflate; server_max_window_bits";
        let mut request = Request::new(());
        request
            .headers_mut()
            .insert(SEC_WEBSOCKET_EXTENSIONS, HeaderValue::from_static(s));

        let mut response = Response::new(());
        let initial_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(11)
            .build();

        let mut parsed_config = initial_config.clone();

        let r = on_receive_request(&request, &mut response, &mut parsed_config);

        assert_eq!(r, Err(DeflateExtensionError::InvalidMaxWindowBits));
        assert_eq!(initial_config, parsed_config);
    }

    #[test]
    fn config_unchanged_on_mismatch() {
        let s = "permessage-deflate; unknown_parameter=\"invalid\"; client_no_context_takeover; server_no_context_takeover";
        let mut request = Request::new(());
        request
            .headers_mut()
            .insert(SEC_WEBSOCKET_EXTENSIONS, HeaderValue::from_static(s));

        let mut response = Response::new(());
        let initial_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(8)
            .build();

        let mut parsed_config = initial_config.clone();

        let r = on_receive_request(&request, &mut response, &mut parsed_config);

        assert_eq!(
            r,
            Err(DeflateExtensionError::NegotiationError(
                "Unknown permessage-deflate parameter: unknown_parameter=\"invalid\"".into()
            ))
        );
        assert_eq!(initial_config, parsed_config);
    }

    #[test]
    fn parses_named_parameters() {
        let s = "permessage-deflate; client_no_context_takeover; client_max_window_bits; server_no_context_takeover; server_max_window_bits=\"8\"";
        let mut request = Request::new(());
        request
            .headers_mut()
            .insert(SEC_WEBSOCKET_EXTENSIONS, HeaderValue::from_static(s));

        let mut response = Response::new(());
        let mut parsed_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(11)
            .build();

        let r = on_receive_request(&request, &mut response, &mut parsed_config);

        assert_eq!(r, Ok(true));

        let mut expected_config = DeflateConfigBuilder::default()
            .server_max_window_bits(8)
            .client_max_window_bits(11)
            .build();

        expected_config.set_compress_reset(true);
        expected_config.set_decompress_reset(true);

        assert_eq!(parsed_config, expected_config);

        let parsed_header = response
            .headers()
            .get(SEC_WEBSOCKET_EXTENSIONS)
            .expect("Missing header")
            .to_str()
            .expect("Failed to parse header");

        assert_eq!(parsed_header, "permessage-deflate; client_no_context_takeover; client_max_window_bits=11; server_no_context_takeover; server_max_window_bits=8");
    }

    #[test]
    fn splits() {
        let s = "not-permessage-deflate; client_no_context_takeover; client_max_window_bits; server_no_context_takeover; server_max_window_bits=8, no-permessage-deflate; client_no_context_takeover; client_max_window_bits; server_no_context_takeover, permessage-deflate; client_no_context_takeover; client_max_window_bits";
        let mut request = Request::new(());
        request
            .headers_mut()
            .insert(SEC_WEBSOCKET_EXTENSIONS, HeaderValue::from_static(s));

        let mut response = Response::new(());
        let mut parsed_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(11)
            .build();

        let r = on_receive_request(&request, &mut response, &mut parsed_config);

        assert_eq!(r, Ok(true));

        let mut expected_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(11)
            .build();
        expected_config.set_decompress_reset(true);

        assert_eq!(parsed_config, expected_config);

        let parsed_header = response
            .headers()
            .get(SEC_WEBSOCKET_EXTENSIONS)
            .expect("Missing header")
            .to_str()
            .expect("Failed to parse header");

        assert_eq!(parsed_header, "permessage-deflate; client_no_context_takeover; client_max_window_bits=11; server_max_window_bits=10");
    }

    #[test]
    fn splits_on_new_line() {
        let s = "not-permessage-deflate; client_no_context_takeover; client_max_window_bits; server_no_context_takeover; server_max_window_bits=8,\\n\\r\\t \\ no-permessage-deflate; client_no_context_takeover; client_max_window_bits; server_no_context_takeover, permessage-deflate; client_no_context_takeover; client_max_window_bits";
        let mut request = Request::new(());
        request
            .headers_mut()
            .insert(SEC_WEBSOCKET_EXTENSIONS, HeaderValue::from_static(s));

        let mut response = Response::new(());
        let mut parsed_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(11)
            .build();

        let r = on_receive_request(&request, &mut response, &mut parsed_config);

        assert_eq!(r, Ok(true));

        let mut expected_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(11)
            .build();
        expected_config.set_decompress_reset(true);

        assert_eq!(parsed_config, expected_config);

        let parsed_header = response
            .headers()
            .get(SEC_WEBSOCKET_EXTENSIONS)
            .expect("Missing header")
            .to_str()
            .expect("Failed to parse header");

        assert_eq!(parsed_header, "permessage-deflate; client_no_context_takeover; client_max_window_bits=11; server_max_window_bits=10");
    }
}

mod client {
    use super::*;
    use crate::extensions::compression::deflate::on_response;

    #[test]
    fn splits_on_new_line() {
        let s = "permessage-deflate; client_no_context_takeover; client_max_window_bits=8; server_max_window_bits=10";

        let mut response = Response::new(());
        response
            .headers_mut()
            .insert(SEC_WEBSOCKET_EXTENSIONS, HeaderValue::from_static(s));

        let mut parsed_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(11)
            .build();

        let r = on_response(&mut response, &mut parsed_config);

        assert_eq!(r, Ok(true));

        let mut expected_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(8)
            .build();
        expected_config.set_compress_reset(true);

        assert_eq!(parsed_config, expected_config);
    }

    #[test]
    fn parses_named_parameters() {
        let s = "permessage-deflate; client_no_context_takeover; client_max_window_bits; server_no_context_takeover; server_max_window_bits=\"8\"";
        let mut request = Request::new(());
        request
            .headers_mut()
            .insert(SEC_WEBSOCKET_EXTENSIONS, HeaderValue::from_static(s));

        let mut response = Response::new(());
        let mut parsed_config = DeflateConfigBuilder::default()
            .server_max_window_bits(10)
            .client_max_window_bits(11)
            .build();

        let r = on_receive_request(&request, &mut response, &mut parsed_config);

        assert_eq!(r, Ok(true));

        let mut expected_config = DeflateConfigBuilder::default()
            .server_max_window_bits(8)
            .client_max_window_bits(11)
            .build();

        expected_config.set_compress_reset(true);
        expected_config.set_decompress_reset(true);

        assert_eq!(parsed_config, expected_config);

        let parsed_header = response
            .headers()
            .get(SEC_WEBSOCKET_EXTENSIONS)
            .expect("Missing header")
            .to_str()
            .expect("Failed to parse header");

        assert_eq!(parsed_header, "permessage-deflate; client_no_context_takeover; client_max_window_bits=11; server_no_context_takeover; server_max_window_bits=8");
    }
}
