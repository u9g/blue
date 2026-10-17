//! Size-delimited protobuf framing for the daemon <-> bridge stdio link: each frame is a varint
//! length followed by one encoded message. This is the same format as
//! `sizeDelimitedEncode` / `sizeDelimitedDecodeStream` in `@bufbuild/protobuf/wire`.

use std::io;

use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Upper bound on a single frame. SDK messages can carry large tool results, but anything
/// past this is a corrupted stream rather than a real message.
pub const MAX_FRAME_LEN: u64 = 64 * 1024 * 1024;

/// Writes one frame and flushes.
pub async fn write_frame<W, M>(writer: &mut W, message: &M) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    M: Message,
{
    writer
        .write_all(&message.encode_length_delimited_to_vec())
        .await?;
    writer.flush().await
}

/// Reads one frame. Returns `Ok(None)` on a clean EOF between frames.
///
/// Reads the length prefix a byte at a time, so wrap raw pipes in a `BufReader`.
pub async fn read_frame<R, M>(reader: &mut R) -> io::Result<Option<M>>
where
    R: AsyncRead + Unpin,
    M: Message + Default,
{
    let Some(len) = read_varint(reader).await? else {
        return Ok(None);
    };
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len} exceeds {MAX_FRAME_LEN}"),
        ));
    }
    let mut buf = vec![0; len as usize];
    reader.read_exact(&mut buf).await?;
    M::decode(buf.as_slice())
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

async fn read_varint<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<u64>> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let mut byte = [0u8; 1];
        if reader.read(&mut byte).await? == 0 {
            return if shift == 0 {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
        }
        value |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(Some(value));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "varint length prefix is too long",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::v1::{BridgeOutput, Ready, SdkMessage, bridge_output::Event};
    use tokio::io::BufReader;

    fn ready() -> BridgeOutput {
        BridgeOutput {
            event: Some(Event::Ready(Ready {
                protocol_version: crate::PROTOCOL_VERSION,
                bridge_version: "0.1.0".into(),
                sdk_version: "0.3.302".into(),
                runtime_version: "1.2.21".into(),
            })),
        }
    }

    #[tokio::test]
    async fn round_trips_frames_then_clean_eof() {
        let big = BridgeOutput {
            event: Some(Event::SdkMessage(SdkMessage {
                json: "x".repeat(300_000),
            })),
        };
        let (mut tx, rx) = tokio::io::duplex(1024);
        let writer = {
            let big = big.clone();
            tokio::spawn(async move {
                write_frame(&mut tx, &ready()).await.unwrap();
                write_frame(&mut tx, &big).await.unwrap();
            })
        };
        let mut rx = BufReader::new(rx);
        assert_eq!(
            read_frame::<_, BridgeOutput>(&mut rx).await.unwrap(),
            Some(ready())
        );
        assert_eq!(
            read_frame::<_, BridgeOutput>(&mut rx).await.unwrap(),
            Some(big)
        );
        writer.await.unwrap();
        assert_eq!(read_frame::<_, BridgeOutput>(&mut rx).await.unwrap(), None);
    }

    #[tokio::test]
    async fn truncated_frame_is_an_error() {
        let bytes = ready().encode_length_delimited_to_vec();
        let mut rx = &bytes[..bytes.len() - 1];
        let err = read_frame::<_, BridgeOutput>(&mut rx).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn oversized_length_is_rejected() {
        let mut bytes = Vec::new();
        prost::encoding::encode_varint(MAX_FRAME_LEN + 1, &mut bytes);
        let mut rx = &bytes[..];
        let err = read_frame::<_, BridgeOutput>(&mut rx).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
