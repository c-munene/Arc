#![forbid(unsafe_code)]

use bytes::Bytes;

use crate::error::{H2Code, H2Error};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub name: Bytes,
    pub value: Bytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderBlockLimits {
    pub max_headers: usize,
    pub max_header_block_bytes: usize,
}

impl Default for HeaderBlockLimits {
    fn default() -> Self {
        Self {
            max_headers: 256,
            max_header_block_bytes: 64 * 1024,
        }
    }
}

pub struct HpackEncoder {
    inner: hpack::Encoder<'static>,
}

impl std::fmt::Debug for HpackEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HpackEncoder(..)")
    }
}

impl Default for HpackEncoder {
    fn default() -> Self {
        Self {
            inner: hpack::Encoder::new(),
        }
    }
}

impl HpackEncoder {
    pub fn encode_headers(&mut self, headers: &[Header]) -> Bytes {
        let encoded = self
            .inner
            .encode(headers.iter().map(|h| (h.name.as_ref(), h.value.as_ref())));
        Bytes::from(encoded)
    }
}

pub struct HpackDecoder {
    inner: hpack::Decoder<'static>,
    limits: HeaderBlockLimits,
}

impl std::fmt::Debug for HpackDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HpackDecoder")
            .field("limits", &self.limits)
            .finish()
    }
}

impl HpackDecoder {
    pub fn new(limits: HeaderBlockLimits) -> Self {
        Self {
            inner: hpack::Decoder::new(),
            limits,
        }
    }

    pub fn decode(&mut self, block: &[u8]) -> Result<Vec<Header>, H2Error> {
        if block.len() > self.limits.max_header_block_bytes {
            return Err(H2Error::new(
                H2Code::FrameSizeError,
                "header block too large",
            ));
        }

        let headers = self.inner.decode(block).map_err(|e| {
            H2Error::new(
                H2Code::CompressionError,
                format!("hpack decode failed: {e:?}"),
            )
        })?;

        if headers.len() > self.limits.max_headers {
            return Err(H2Error::new(
                H2Code::ProtocolError,
                "too many headers in block",
            ));
        }

        let mut out = Vec::with_capacity(headers.len());
        for (name, value) in headers {
            out.push(Header {
                name: Bytes::from(name),
                value: Bytes::from(value),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hpack_roundtrip() {
        let mut enc = HpackEncoder::default();
        let headers = vec![
            Header {
                name: Bytes::from_static(b":method"),
                value: Bytes::from_static(b"GET"),
            },
            Header {
                name: Bytes::from_static(b":path"),
                value: Bytes::from_static(b"/x"),
            },
            Header {
                name: Bytes::from_static(b"host"),
                value: Bytes::from_static(b"example.com"),
            },
        ];
        let block = enc.encode_headers(&headers);
        let mut dec = HpackDecoder::new(HeaderBlockLimits::default());
        let out = dec.decode(&block).expect("decode");
        assert_eq!(out.len(), headers.len());
        for (a, b) in out.iter().zip(headers.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.value, b.value);
        }
    }
}
