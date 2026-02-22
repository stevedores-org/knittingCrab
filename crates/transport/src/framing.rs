use crate::error::{TransportError, TransportResult};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024; // 16 MiB

/// Wire format: [u32 LE length][JSON payload]
pub struct FramedTransport {
    stream: TcpStream,
}

impl FramedTransport {
    pub fn new(stream: TcpStream) -> Self {
        Self { stream }
    }

    /// Send a message. Serializes to JSON and prepends length prefix.
    pub async fn send_message<T: Serialize>(&mut self, msg: &T) -> TransportResult<()> {
        let json = serde_json::to_vec(msg)?;

        if json.len() > MAX_MESSAGE_BYTES {
            return Err(TransportError::MessageTooLarge(json.len()));
        }

        let len = json.len() as u32;
        let mut frame = Vec::with_capacity(4 + json.len());
        frame.extend_from_slice(&len.to_le_bytes());
        frame.extend_from_slice(&json);

        self.stream.write_all(&frame).await?;
        Ok(())
    }

    /// Receive a message. Returns Ok(None) for clean EOF on length prefix, Err for protocol errors.
    pub async fn recv_message<T: for<'de> Deserialize<'de>>(
        &mut self,
    ) -> TransportResult<Option<T>> {
        let mut len_bytes = [0u8; 4];
        match self.stream.read_exact(&mut len_bytes).await {
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // Clean EOF on length prefix
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }

        let len = u32::from_le_bytes(len_bytes) as usize;

        if len > MAX_MESSAGE_BYTES {
            return Err(TransportError::MessageTooLarge(len));
        }

        let mut payload = vec![0u8; len];
        self.stream.read_exact(&mut payload).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                TransportError::Protocol("truncated frame".to_string())
            } else {
                e.into()
            }
        })?;

        let msg = serde_json::from_slice(&payload)?;
        Ok(Some(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct TestMessage {
        id: u32,
        text: String,
    }

    #[tokio::test]
    async fn framing_round_trip_over_loopback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut transport = FramedTransport::new(stream);

            let msg: Option<TestMessage> = transport.recv_message().await.unwrap();
            assert_eq!(
                msg,
                Some(TestMessage {
                    id: 42,
                    text: "hello".to_string()
                })
            );

            transport
                .send_message(&TestMessage {
                    id: 100,
                    text: "world".to_string(),
                })
                .await
                .unwrap();
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut transport = FramedTransport::new(stream);

        transport
            .send_message(&TestMessage {
                id: 42,
                text: "hello".to_string(),
            })
            .await
            .unwrap();

        let response: Option<TestMessage> = transport.recv_message().await.unwrap();
        assert_eq!(
            response,
            Some(TestMessage {
                id: 100,
                text: "world".to_string()
            })
        );

        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn clean_eof_returns_none() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            // Server immediately drops connection (clean EOF)
            drop(stream);
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut transport = FramedTransport::new(stream);

        let result: TransportResult<Option<TestMessage>> = transport.recv_message().await;
        assert!(matches!(result, Ok(None)));
    }

    #[tokio::test]
    async fn oversized_message_rejected() {
        let msg = TestMessage {
            id: 1,
            text: "x".repeat(20 * 1024 * 1024), // 20 MiB
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            // Server doesn't read anything
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut transport = FramedTransport::new(stream);

        let result = transport.send_message(&msg).await;
        assert!(matches!(result, Err(TransportError::MessageTooLarge(_))));
    }

    #[test]
    fn round_trip_all_request_variants() {
        // Tested via messages.rs tests; this is a placeholder
        // to ensure compile-time serialization works
        use crate::messages::CoordinatorRequest;
        use knitting_crab_core::ids::WorkerId;
        let req = CoordinatorRequest::Dequeue {
            worker_id: WorkerId::new(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let _deserialized: CoordinatorRequest = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn round_trip_all_response_variants() {
        use crate::messages::CoordinatorResponse;
        let resp = CoordinatorResponse::Ok;
        let json = serde_json::to_string(&resp).unwrap();
        let _deserialized: CoordinatorResponse = serde_json::from_str(&json).unwrap();
    }
}
