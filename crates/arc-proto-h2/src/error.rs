#![forbid(unsafe_code)]

use std::error::Error as StdError;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum H2Code {
    NoError = 0x0,
    ProtocolError = 0x1,
    InternalError = 0x2,
    FlowControlError = 0x3,
    SettingsTimeout = 0x4,
    StreamClosed = 0x5,
    FrameSizeError = 0x6,
    RefusedStream = 0x7,
    Cancel = 0x8,
    CompressionError = 0x9,
    ConnectError = 0xa,
    EnhanceYourCalm = 0xb,
    InadequateSecurity = 0xc,
    Http11Required = 0xd,
}

impl H2Code {
    #[inline]
    pub fn as_u32(self) -> u32 {
        self as u32
    }

    #[inline]
    pub fn from_u32(v: u32) -> Self {
        match v {
            0x0 => Self::NoError,
            0x1 => Self::ProtocolError,
            0x2 => Self::InternalError,
            0x3 => Self::FlowControlError,
            0x4 => Self::SettingsTimeout,
            0x5 => Self::StreamClosed,
            0x6 => Self::FrameSizeError,
            0x7 => Self::RefusedStream,
            0x8 => Self::Cancel,
            0x9 => Self::CompressionError,
            0xa => Self::ConnectError,
            0xb => Self::EnhanceYourCalm,
            0xc => Self::InadequateSecurity,
            0xd => Self::Http11Required,
            _ => Self::InternalError,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct H2Error {
    pub code: H2Code,
    pub message: String,
}

impl H2Error {
    #[inline]
    pub fn new(code: H2Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for H2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "h2 {:?}: {}", self.code, self.message)
    }
}

impl StdError for H2Error {}
