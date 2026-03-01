#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::io::{self, Write};

use super::buf::BufOps;
use super::tx::{Credit, TxItem};

pub fn drain_tx_to_writer<W: Write>(
    tx: &mut VecDeque<TxItem>,
    ops: &mut dyn BufOps,
    w: &mut W,
    mut on_credit: impl FnMut(Credit),
    max_plain_bytes: usize,
) -> io::Result<usize> {
    let mut wrote: usize = 0;

    while let Some(item) = tx.front_mut() {
        // 尽量不超过 max_plain_bytes，但为了避免 starvation：如果 wrote==0，允许至少写一个 item
        let item_len = match item {
            #[cfg(feature = "h2-native-upstream")]
            TxItem::Raw { bytes } => bytes.len(),
            TxItem::FrameBytes { payload, .. } => 9 + payload.len(),
            TxItem::FrameData { payload, .. } => 9 + (payload.total_len() as usize),
        };

        if wrote > 0 && wrote + item_len > max_plain_bytes {
            break;
        }

        match item {
            #[cfg(feature = "h2-native-upstream")]
            TxItem::Raw { bytes } => {
                w.write_all(bytes)?;
                wrote += bytes.len();
                tx.pop_front();
            }

            TxItem::FrameBytes { header, payload } => {
                w.write_all(header)?;
                w.write_all(payload)?;
                wrote += 9 + payload.len();
                tx.pop_front();
            }

            TxItem::FrameData {
                header,
                payload,
                credit,
            } => {
                w.write_all(header)?;
                wrote += 9;

                for seg in payload.iter() {
                    let s = ops.slice(seg.buf_id, seg.off, seg.len);
                    w.write_all(s)?;
                    wrote += s.len();
                }

                // 成功写入 rustls writer 后：释放 payload（零拷贝链路结束点）
                payload.release(ops);

                // 写完立刻触发 credit：WINDOW_UPDATE 背压闭环
                if let Some(c) = credit.take() {
                    on_credit(c);
                }

                tx.pop_front();
            }
        }
    }

    Ok(wrote)
}
