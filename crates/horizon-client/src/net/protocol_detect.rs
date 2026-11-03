use super::protocol_compat::ProtocolVersion;
use horizon_protocol::{
    PROTOCOL_VERSION,
    connection::Connection,
    packets::{Empty, handshake, status},
};
use std::time::Duration;

pub const DETECT_TIMEOUT: Duration = Duration::from_secs(5);

pub fn detect(host: &str, port: u16, timeout: Duration) -> Result<ProtocolVersion, String> {
    let mut connection = Connection::connect_server(host, port, timeout)
        .map_err(|e| format!("could not reach the server: {e}"))?;
    connection
        .reader
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("could not set the status timeout: {e}"))?;
    let e = |e: horizon_protocol::Error| format!("status ping failed: {e}");
    connection
        .send(
            &handshake::Serverbound::Intention(handshake::Intention {
                protocol: PROTOCOL_VERSION,
                host: host.to_owned(),
                port,
                intent: handshake::STATUS,
            })
            .encode(),
        )
        .map_err(e)?;
    connection
        .send(&status::Serverbound::StatusRequest(Empty).encode())
        .map_err(e)?;
    let (id, payload) = connection.recv().map_err(e)?;
    let json = match status::Clientbound::decode(id, &payload).map_err(e)? {
        status::Clientbound::StatusResponse(response) => response.json,
        other => return Err(format!("unexpected status reply: {other:?}")),
    };
    let status: serde_json::Value = serde_json::from_str(&json)
        .map_err(|e| format!("the server sent malformed status JSON: {e}"))?;
    let raw = status
        .get("version")
        .and_then(|version| version.get("protocol"))
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| "the server status has no version.protocol".to_owned())?;
    let raw = i32::try_from(raw)
        .map_err(|_| format!("the server reported an out-of-range protocol {raw}"))?;
    Ok(ProtocolVersion::new(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::packets::status::StatusResponse;
    use std::{
        net::TcpListener,
        thread::{self, JoinHandle},
    };

    fn mock_status(json: &'static str) -> (u16, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let thread = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut connection = Connection::from_stream(stream).unwrap();
            let (id, body) = connection.recv().unwrap();
            assert!(matches!(
                handshake::Serverbound::decode(id, &body).unwrap(),
                handshake::Serverbound::Intention(handshake::Intention {
                    intent: handshake::STATUS,
                    ..
                })
            ));
            let (id, body) = connection.recv().unwrap();
            assert_eq!(
                status::Serverbound::decode(id, &body).unwrap(),
                status::Serverbound::StatusRequest(Empty)
            );
            connection
                .send(
                    &status::Clientbound::StatusResponse(StatusResponse {
                        json: json.to_owned(),
                    })
                    .encode(),
                )
                .unwrap();
        });
        (port, thread)
    }

    #[test]
    fn reads_the_reported_protocol_from_a_status_ping() {
        let (port, server) = mock_status(r#"{"version":{"name":"26.2","protocol":776}}"#);
        assert_eq!(
            detect("127.0.0.1", port, Duration::from_secs(2)).unwrap(),
            ProtocolVersion::MINECRAFT_26_2
        );
        server.join().unwrap();
    }

    #[test]
    fn reports_a_missing_protocol_instead_of_assuming_native() {
        let (port, server) = mock_status(r#"{"version":{"name":"unknown"}}"#);
        let error = detect("127.0.0.1", port, Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("no version.protocol"), "{error}");
        server.join().unwrap();
    }

    #[test]
    fn reports_malformed_status_json() {
        let (port, server) = mock_status("not json");
        let error = detect("127.0.0.1", port, Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("malformed status JSON"), "{error}");
        server.join().unwrap();
    }

    #[test]
    fn times_out_on_a_server_that_never_answers_status() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_millis(500));
        });
        let error = detect("127.0.0.1", port, Duration::from_millis(150)).unwrap_err();
        assert!(error.contains("status ping failed"), "{error}");
        server.join().unwrap();
    }

    #[test]
    fn reports_an_unreachable_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let error = detect("127.0.0.1", port, Duration::from_millis(200)).unwrap_err();
        assert!(error.contains("could not reach the server"), "{error}");
    }
}
