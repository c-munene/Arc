#![forbid(unsafe_code)]

use crate::error::{H2Code, H2Error};

const SETTINGS_HEADER_TABLE_SIZE: u16 = 0x1;
const SETTINGS_ENABLE_PUSH: u16 = 0x2;
const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;
const SETTINGS_MAX_FRAME_SIZE: u16 = 0x5;
const SETTINGS_MAX_HEADER_LIST_SIZE: u16 = 0x6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub header_table_size: u32,
    pub enable_push: bool,
    pub max_concurrent_streams: u32,
    pub initial_window_size: u32,
    pub max_frame_size: u32,
    pub max_header_list_size: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            header_table_size: 4096,
            enable_push: true,
            max_concurrent_streams: 1024,
            initial_window_size: 65_535,
            max_frame_size: 16_384,
            max_header_list_size: 64 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsDelta {
    initial_window_delta: i64,
}

impl SettingsDelta {
    #[inline]
    pub fn initial_window_delta(self) -> i64 {
        self.initial_window_delta
    }
}

impl Settings {
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        fn push(out: &mut Vec<u8>, id: u16, v: u32) {
            out.extend_from_slice(&id.to_be_bytes());
            out.extend_from_slice(&v.to_be_bytes());
        }

        push(out, SETTINGS_HEADER_TABLE_SIZE, self.header_table_size);
        push(
            out,
            SETTINGS_ENABLE_PUSH,
            if self.enable_push { 1 } else { 0 },
        );
        push(
            out,
            SETTINGS_MAX_CONCURRENT_STREAMS,
            self.max_concurrent_streams,
        );
        push(out, SETTINGS_INITIAL_WINDOW_SIZE, self.initial_window_size);
        push(out, SETTINGS_MAX_FRAME_SIZE, self.max_frame_size);
        push(
            out,
            SETTINGS_MAX_HEADER_LIST_SIZE,
            self.max_header_list_size,
        );
    }

    pub fn apply_from_payload(&mut self, payload: &[u8]) -> Result<SettingsDelta, H2Error> {
        if payload.len() % 6 != 0 {
            return Err(H2Error::new(
                H2Code::FrameSizeError,
                "SETTINGS payload length must be multiple of 6",
            ));
        }

        let old_initial = self.initial_window_size;
        let mut i = 0usize;
        while i < payload.len() {
            let id = u16::from_be_bytes([payload[i], payload[i + 1]]);
            let val = u32::from_be_bytes([
                payload[i + 2],
                payload[i + 3],
                payload[i + 4],
                payload[i + 5],
            ]);
            i += 6;

            match id {
                SETTINGS_HEADER_TABLE_SIZE => {
                    self.header_table_size = val;
                }
                SETTINGS_ENABLE_PUSH => match val {
                    0 => self.enable_push = false,
                    1 => self.enable_push = true,
                    _ => {
                        return Err(H2Error::new(
                            H2Code::ProtocolError,
                            "SETTINGS_ENABLE_PUSH must be 0 or 1",
                        ))
                    }
                },
                SETTINGS_MAX_CONCURRENT_STREAMS => {
                    self.max_concurrent_streams = val;
                }
                SETTINGS_INITIAL_WINDOW_SIZE => {
                    if val > 0x7fff_ffff {
                        return Err(H2Error::new(
                            H2Code::FlowControlError,
                            "SETTINGS_INITIAL_WINDOW_SIZE exceeds 2^31-1",
                        ));
                    }
                    self.initial_window_size = val;
                }
                SETTINGS_MAX_FRAME_SIZE => {
                    if !(16_384..=16_777_215).contains(&val) {
                        return Err(H2Error::new(
                            H2Code::ProtocolError,
                            "SETTINGS_MAX_FRAME_SIZE out of range",
                        ));
                    }
                    self.max_frame_size = val;
                }
                SETTINGS_MAX_HEADER_LIST_SIZE => {
                    self.max_header_list_size = val;
                }
                _ => {
                    // Unknown settings are ignored per RFC7540.
                }
            }
        }

        Ok(SettingsDelta {
            initial_window_delta: self.initial_window_size as i64 - old_initial as i64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_settings_and_delta() {
        let mut s = Settings::default();
        let mut p = Vec::new();
        // initial_window_size=70000
        p.extend_from_slice(&SETTINGS_INITIAL_WINDOW_SIZE.to_be_bytes());
        p.extend_from_slice(&(70_000u32).to_be_bytes());
        // max_frame_size=32768
        p.extend_from_slice(&SETTINGS_MAX_FRAME_SIZE.to_be_bytes());
        p.extend_from_slice(&(32_768u32).to_be_bytes());

        let d = s.apply_from_payload(&p).expect("apply");
        assert_eq!(s.initial_window_size, 70_000);
        assert_eq!(s.max_frame_size, 32_768);
        assert_eq!(d.initial_window_delta(), 70_000 - 65_535);
    }

    #[test]
    fn invalid_initial_window_rejected() {
        let mut s = Settings::default();
        let mut p = Vec::new();
        p.extend_from_slice(&SETTINGS_INITIAL_WINDOW_SIZE.to_be_bytes());
        p.extend_from_slice(&(0x8000_0000u32).to_be_bytes());
        let e = s.apply_from_payload(&p).unwrap_err();
        assert_eq!(e.code, H2Code::FlowControlError);
    }
}
