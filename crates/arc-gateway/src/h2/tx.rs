#![forbid(unsafe_code)]

use bytes::Bytes;

use super::buf::BufChain;
use super::key::ConnKey;

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub enum Credit {
    ToDownstream {
        conn: ConnKey,
        sid: u32,
        bytes: u32,
    },
    #[cfg(feature = "h2-native-upstream")]
    ToUpstream {
        conn: ConnKey,
        sid: u32,
        bytes: u32,
    },
}

#[derive(Debug)]
pub enum TxItem {
    /// 仅 upstream client 需要：client connection preface（不是 frame）
    #[cfg(feature = "h2-native-upstream")]
    Raw { bytes: Bytes },

    /// 普通 frame：9B header + 小 payload（Bytes）
    FrameBytes { header: [u8; 9], payload: Bytes },

    /// DATA frame：9B header + 零拷贝 payload（BufChain）
    /// credit：当此 payload 被写入 rustls writer 成功后触发（用于 WINDOW_UPDATE 闭环）
    FrameData {
        header: [u8; 9],
        payload: BufChain,
        credit: Option<Credit>,
    },
}
