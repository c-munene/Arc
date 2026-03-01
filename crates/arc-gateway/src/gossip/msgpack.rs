use std::fmt;

/// Result type for MsgPack codec operations.
pub type MsgPackResult<T> = std::result::Result<T, MsgPackError>;

/// Minimal MessagePack codec error.
#[derive(Debug)]
pub enum MsgPackError {
    /// Unexpected end of input.
    Eof,
    /// Type mismatch.
    InvalidType { expected: &'static str, got: u8 },
    /// Malformed or semantically invalid input.
    InvalidData(&'static str),
    /// UTF-8 decode error.
    Utf8,
}

impl fmt::Display for MsgPackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MsgPackError::Eof => write!(f, "msgpack: unexpected eof"),
            MsgPackError::InvalidType { expected, got } => {
                write!(
                    f,
                    "msgpack: invalid type (expected {expected}, got 0x{got:02x})"
                )
            }
            MsgPackError::InvalidData(msg) => write!(f, "msgpack: invalid data: {msg}"),
            MsgPackError::Utf8 => write!(f, "msgpack: invalid utf-8"),
        }
    }
}

impl std::error::Error for MsgPackError {}

/// MessagePack encoder (subset needed by Arc gossip bus).
#[derive(Debug, Default)]
pub struct Encoder {
    buf: Vec<u8>,
}

impl Encoder {
    #[inline]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }

    #[inline]
    pub fn write_nil(&mut self) {
        self.buf.push(0xc0);
    }

    #[inline]
    pub fn write_u64(&mut self, v: u64) {
        if v <= 0x7f {
            self.buf.push(v as u8);
        } else if v <= u64::from(u8::MAX) {
            self.buf.push(0xcc);
            self.buf.push(v as u8);
        } else if v <= u64::from(u16::MAX) {
            self.buf.push(0xcd);
            self.buf.extend_from_slice(&(v as u16).to_be_bytes());
        } else if v <= u64::from(u32::MAX) {
            self.buf.push(0xce);
            self.buf.extend_from_slice(&(v as u32).to_be_bytes());
        } else {
            self.buf.push(0xcf);
            self.buf.extend_from_slice(&v.to_be_bytes());
        }
    }

    #[inline]
    pub fn write_array_len(&mut self, len: usize) {
        if len <= 15 {
            self.buf.push(0x90 | (len as u8));
        } else if len <= usize::from(u16::MAX) {
            self.buf.push(0xdc);
            self.buf.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            self.buf.push(0xdd);
            self.buf.extend_from_slice(&(len as u32).to_be_bytes());
        }
    }

    #[inline]
    pub fn write_map_len(&mut self, len: usize) {
        if len <= 15 {
            self.buf.push(0x80 | (len as u8));
        } else if len <= usize::from(u16::MAX) {
            self.buf.push(0xde);
            self.buf.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            self.buf.push(0xdf);
            self.buf.extend_from_slice(&(len as u32).to_be_bytes());
        }
    }

    #[inline]
    pub fn write_str(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let len = bytes.len();

        if len <= 31 {
            self.buf.push(0xa0 | (len as u8));
        } else if len <= usize::from(u8::MAX) {
            self.buf.push(0xd9);
            self.buf.push(len as u8);
        } else if len <= usize::from(u16::MAX) {
            self.buf.push(0xda);
            self.buf.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            self.buf.push(0xdb);
            self.buf.extend_from_slice(&(len as u32).to_be_bytes());
        }

        self.buf.extend_from_slice(bytes);
    }

    #[inline]
    pub fn write_bin(&mut self, b: &[u8]) {
        let len = b.len();

        if len <= usize::from(u8::MAX) {
            self.buf.push(0xc4);
            self.buf.push(len as u8);
        } else if len <= usize::from(u16::MAX) {
            self.buf.push(0xc5);
            self.buf.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            self.buf.push(0xc6);
            self.buf.extend_from_slice(&(len as u32).to_be_bytes());
        }

        self.buf.extend_from_slice(b);
    }
}

/// MessagePack decoder (subset needed by Arc gossip bus).
#[derive(Debug)]
pub struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    #[inline]
    pub fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    #[inline]
    pub fn remaining(&self) -> usize {
        self.input.len().saturating_sub(self.pos)
    }

    #[inline]
    pub fn peek_u8(&self) -> MsgPackResult<u8> {
        self.input.get(self.pos).copied().ok_or(MsgPackError::Eof)
    }

    #[inline]
    fn read_u8(&mut self) -> MsgPackResult<u8> {
        let b = self.input.get(self.pos).copied().ok_or(MsgPackError::Eof)?;
        self.pos += 1;
        Ok(b)
    }

    #[inline]
    fn read_exact(&mut self, n: usize) -> MsgPackResult<&'a [u8]> {
        if self.remaining() < n {
            return Err(MsgPackError::Eof);
        }
        let start = self.pos;
        self.pos += n;
        Ok(&self.input[start..start + n])
    }

    #[inline]
    pub fn read_nil(&mut self) -> MsgPackResult<()> {
        let b = self.read_u8()?;
        if b == 0xc0 {
            Ok(())
        } else {
            Err(MsgPackError::InvalidType {
                expected: "nil",
                got: b,
            })
        }
    }

    #[inline]
    pub fn read_u64(&mut self) -> MsgPackResult<u64> {
        let b = self.read_u8()?;
        if b <= 0x7f {
            return Ok(u64::from(b));
        }

        match b {
            0xcc => Ok(u64::from(self.read_u8()?)),
            0xcd => {
                let v = self.read_exact(2)?;
                Ok(u64::from(u16::from_be_bytes([v[0], v[1]])))
            }
            0xce => {
                let v = self.read_exact(4)?;
                Ok(u64::from(u32::from_be_bytes([v[0], v[1], v[2], v[3]])))
            }
            0xcf => {
                let v = self.read_exact(8)?;
                Ok(u64::from_be_bytes([
                    v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7],
                ]))
            }
            _ => Err(MsgPackError::InvalidType {
                expected: "u64",
                got: b,
            }),
        }
    }

    #[inline]
    pub fn read_array_len(&mut self) -> MsgPackResult<usize> {
        let b = self.read_u8()?;
        if (b & 0xf0) == 0x90 {
            return Ok(usize::from(b & 0x0f));
        }
        match b {
            0xdc => {
                let v = self.read_exact(2)?;
                Ok(usize::from(u16::from_be_bytes([v[0], v[1]])))
            }
            0xdd => {
                let v = self.read_exact(4)?;
                Ok(u32::from_be_bytes([v[0], v[1], v[2], v[3]]) as usize)
            }
            _ => Err(MsgPackError::InvalidType {
                expected: "array",
                got: b,
            }),
        }
    }

    #[inline]
    pub fn read_map_len(&mut self) -> MsgPackResult<usize> {
        let b = self.read_u8()?;
        if (b & 0xf0) == 0x80 {
            return Ok(usize::from(b & 0x0f));
        }
        match b {
            0xde => {
                let v = self.read_exact(2)?;
                Ok(usize::from(u16::from_be_bytes([v[0], v[1]])))
            }
            0xdf => {
                let v = self.read_exact(4)?;
                Ok(u32::from_be_bytes([v[0], v[1], v[2], v[3]]) as usize)
            }
            _ => Err(MsgPackError::InvalidType {
                expected: "map",
                got: b,
            }),
        }
    }

    #[inline]
    pub fn read_str(&mut self) -> MsgPackResult<String> {
        let b = self.read_u8()?;
        let len: usize = if (b & 0xe0) == 0xa0 {
            usize::from(b & 0x1f)
        } else {
            match b {
                0xd9 => usize::from(self.read_u8()?),
                0xda => {
                    let v = self.read_exact(2)?;
                    usize::from(u16::from_be_bytes([v[0], v[1]]))
                }
                0xdb => {
                    let v = self.read_exact(4)?;
                    u32::from_be_bytes([v[0], v[1], v[2], v[3]]) as usize
                }
                _ => {
                    return Err(MsgPackError::InvalidType {
                        expected: "str",
                        got: b,
                    })
                }
            }
        };

        let bytes = self.read_exact(len)?;
        let s = std::str::from_utf8(bytes).map_err(|_| MsgPackError::Utf8)?;
        Ok(s.to_string())
    }

    #[inline]
    pub fn read_bin(&mut self) -> MsgPackResult<Vec<u8>> {
        let b = self.read_u8()?;
        let len: usize = match b {
            0xc4 => usize::from(self.read_u8()?),
            0xc5 => {
                let v = self.read_exact(2)?;
                usize::from(u16::from_be_bytes([v[0], v[1]]))
            }
            0xc6 => {
                let v = self.read_exact(4)?;
                u32::from_be_bytes([v[0], v[1], v[2], v[3]]) as usize
            }
            _ => {
                return Err(MsgPackError::InvalidType {
                    expected: "bin",
                    got: b,
                })
            }
        };

        Ok(self.read_exact(len)?.to_vec())
    }
}
