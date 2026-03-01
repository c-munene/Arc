#![forbid(unsafe_code)]

use crate::error::{H2Code, H2Error};

pub const FRAME_HEADER_LEN: usize = 9;
pub const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

pub mod flags {
    pub const ACK: u8 = 0x1;
    pub const END_STREAM: u8 = 0x1;
    pub const END_HEADERS: u8 = 0x4;
    pub const PADDED: u8 = 0x8;
    pub const PRIORITY: u8 = 0x20;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameType {
    Data,
    Headers,
    Priority,
    RstStream,
    Settings,
    PushPromise,
    Ping,
    Goaway,
    WindowUpdate,
    Continuation,
    Unknown(u8),
}

impl FrameType {
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x0 => Self::Data,
            0x1 => Self::Headers,
            0x2 => Self::Priority,
            0x3 => Self::RstStream,
            0x4 => Self::Settings,
            0x5 => Self::PushPromise,
            0x6 => Self::Ping,
            0x7 => Self::Goaway,
            0x8 => Self::WindowUpdate,
            0x9 => Self::Continuation,
            x => Self::Unknown(x),
        }
    }

    #[inline]
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Data => 0x0,
            Self::Headers => 0x1,
            Self::Priority => 0x2,
            Self::RstStream => 0x3,
            Self::Settings => 0x4,
            Self::PushPromise => 0x5,
            Self::Ping => 0x6,
            Self::Goaway => 0x7,
            Self::WindowUpdate => 0x8,
            Self::Continuation => 0x9,
            Self::Unknown(x) => x,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub len: u32,
    pub ty: FrameType,
    pub flags: u8,
    pub stream_id: u32,
}

impl FrameHeader {
    #[inline]
    pub fn write_into(&self, out: &mut [u8; FRAME_HEADER_LEN]) {
        let len = self.len & 0x00ff_ffff;
        out[0] = ((len >> 16) & 0xff) as u8;
        out[1] = ((len >> 8) & 0xff) as u8;
        out[2] = (len & 0xff) as u8;
        out[3] = self.ty.as_u8();
        out[4] = self.flags;
        let sid = self.stream_id & 0x7fff_ffff;
        out[5] = ((sid >> 24) & 0xff) as u8;
        out[6] = ((sid >> 16) & 0xff) as u8;
        out[7] = ((sid >> 8) & 0xff) as u8;
        out[8] = (sid & 0xff) as u8;
    }

    #[inline]
    pub fn parse(inp: &[u8; FRAME_HEADER_LEN], max_frame_size: u32) -> Result<Self, H2Error> {
        let len = ((inp[0] as u32) << 16) | ((inp[1] as u32) << 8) | (inp[2] as u32);
        if len > max_frame_size.min(16_777_215) {
            return Err(H2Error::new(
                H2Code::FrameSizeError,
                "frame payload exceeds peer max_frame_size",
            ));
        }
        let ty = FrameType::from_u8(inp[3]);
        let flags = inp[4];
        let sid = (((inp[5] as u32) << 24)
            | ((inp[6] as u32) << 16)
            | ((inp[7] as u32) << 8)
            | (inp[8] as u32))
            & 0x7fff_ffff;

        Ok(Self {
            len,
            ty,
            flags,
            stream_id: sid,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_header_roundtrip() {
        let h = FrameHeader {
            len: 1024,
            ty: FrameType::Headers,
            flags: flags::END_HEADERS,
            stream_id: 0x7fff_fffe,
        };
        let mut b = [0u8; FRAME_HEADER_LEN];
        h.write_into(&mut b);
        let p = FrameHeader::parse(&b, 16_384).expect("parse");
        assert_eq!(p.len, h.len);
        assert_eq!(p.ty, h.ty);
        assert_eq!(p.flags, h.flags);
        assert_eq!(p.stream_id, h.stream_id);
    }

    #[test]
    fn frame_size_guard_works() {
        let mut b = [0u8; FRAME_HEADER_LEN];
        b[0] = 0;
        b[1] = 0x50;
        b[2] = 0x00; // 20480
        b[3] = FrameType::Data.as_u8();
        let e = FrameHeader::parse(&b, 16_384).unwrap_err();
        assert_eq!(e.code, H2Code::FrameSizeError);
    }
}
