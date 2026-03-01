#![forbid(unsafe_code)]

use std::cmp::min;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufSeg {
    pub buf_id: u16,
    pub off: u32,
    pub len: u32,
}

pub trait BufOps {
    fn slice<'a>(&'a self, buf_id: u16, off: u32, len: u32) -> &'a [u8];
    fn release(&mut self, buf_id: u16);
    fn retain(&mut self, _buf_id: u16) {}
}

#[derive(Debug, Default)]
pub struct BufChain {
    segs: VecDeque<BufSeg>,
    total_len: u32,
}

impl BufChain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.total_len == 0
    }

    pub fn total_len(&self) -> u32 {
        self.total_len
    }

    pub fn iter(&self) -> impl Iterator<Item = &BufSeg> {
        self.segs.iter()
    }

    pub fn push_seg(&mut self, buf_id: u16, off: u32, len: u32) {
        self.push_back(BufSeg { buf_id, off, len });
    }

    fn push_back(&mut self, seg: BufSeg) {
        if seg.len == 0 {
            return;
        }
        self.total_len = self.total_len.saturating_add(seg.len);
        self.segs.push_back(seg);
    }

    pub fn release(&mut self, ops: &mut dyn BufOps) {
        while let Some(seg) = self.segs.pop_front() {
            ops.release(seg.buf_id);
        }
        self.total_len = 0;
    }

    pub fn take_prefix(&mut self, want: u32, ops: &mut dyn BufOps) -> Self {
        if want == 0 || self.total_len == 0 {
            return Self::new();
        }

        let mut out = Self::new();
        let mut remaining = want.min(self.total_len);
        while remaining > 0 {
            let Some(mut seg) = self.segs.pop_front() else {
                break;
            };

            if seg.len <= remaining {
                remaining -= seg.len;
                self.total_len = self.total_len.saturating_sub(seg.len);
                out.push_back(seg);
                continue;
            }

            // Split one segment into two owners: keep tail in self, move head to out.
            let take = remaining;
            let head = BufSeg {
                buf_id: seg.buf_id,
                off: seg.off,
                len: take,
            };

            seg.off = seg.off.saturating_add(take);
            seg.len -= take;

            ops.retain(head.buf_id);
            self.segs.push_front(seg);
            self.total_len = self.total_len.saturating_sub(take);
            out.push_back(head);
            remaining = 0;
        }

        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxChunk {
    pub buf_id: u16,
    pub off: u32,
    pub len: u32,
}

#[derive(Debug, Default)]
pub struct RxQueue {
    q: VecDeque<BufSeg>,
    total: u32,
}

impl RxQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn available(&self) -> u32 {
        self.total
    }

    pub fn release_all(&mut self, ops: &mut dyn BufOps) {
        while let Some(seg) = self.q.pop_front() {
            ops.release(seg.buf_id);
        }
        self.total = 0;
    }

    pub fn push_chunk(&mut self, c: RxChunk) {
        if c.len == 0 {
            return;
        }
        self.q.push_back(BufSeg {
            buf_id: c.buf_id,
            off: c.off,
            len: c.len,
        });
        self.total = self.total.saturating_add(c.len);
    }

    pub fn peek_exact(&self, n: usize, ops: &dyn BufOps, out: &mut [u8]) -> bool {
        if self.total < n as u32 || out.len() < n {
            return false;
        }
        let mut rem = n;
        let mut dst_off = 0usize;
        for seg in &self.q {
            if rem == 0 {
                break;
            }
            let take = min(rem, seg.len as usize);
            let src = ops.slice(seg.buf_id, seg.off, take as u32);
            out[dst_off..dst_off + take].copy_from_slice(src);
            dst_off += take;
            rem -= take;
        }
        rem == 0
    }

    pub fn read_u8(&mut self, ops: &mut dyn BufOps) -> Result<u8, ()> {
        let mut b = [0u8; 1];
        if !self.peek_exact(1, ops, &mut b) {
            return Err(());
        }
        self.consume(1, ops)?;
        Ok(b[0])
    }

    pub fn consume(&mut self, mut n: u32, ops: &mut dyn BufOps) -> Result<(), ()> {
        if self.total < n {
            return Err(());
        }
        while n > 0 {
            let Some(mut seg) = self.q.pop_front() else {
                return Err(());
            };
            if seg.len <= n {
                n -= seg.len;
                self.total -= seg.len;
                ops.release(seg.buf_id);
            } else {
                seg.off = seg.off.saturating_add(n);
                seg.len -= n;
                self.total -= n;
                n = 0;
                self.q.push_front(seg);
            }
        }
        Ok(())
    }

    pub fn copy_into_vec(
        &mut self,
        mut n: u32,
        ops: &mut dyn BufOps,
        out: &mut Vec<u8>,
    ) -> Result<(), ()> {
        if self.total < n {
            return Err(());
        }
        out.reserve(n as usize);
        while n > 0 {
            let Some(mut seg) = self.q.pop_front() else {
                return Err(());
            };
            let take = min(n, seg.len);
            let src = ops.slice(seg.buf_id, seg.off, take);
            out.extend_from_slice(src);

            if take == seg.len {
                self.total -= take;
                n -= take;
                ops.release(seg.buf_id);
            } else {
                seg.off = seg.off.saturating_add(take);
                seg.len -= take;
                self.total -= take;
                n -= take;
                self.q.push_front(seg);
            }
        }
        Ok(())
    }

    pub fn take_chain(&mut self, mut n: u32, ops: &mut dyn BufOps) -> Result<BufChain, ()> {
        if self.total < n {
            return Err(());
        }
        let mut out = BufChain::new();
        while n > 0 {
            let Some(mut seg) = self.q.pop_front() else {
                return Err(());
            };

            if seg.len <= n {
                n -= seg.len;
                self.total -= seg.len;
                out.push_back(seg);
                continue;
            }

            // Split queue head: queue keeps tail, chain gets prefix.
            let take = n;
            let head = BufSeg {
                buf_id: seg.buf_id,
                off: seg.off,
                len: take,
            };
            seg.off = seg.off.saturating_add(take);
            seg.len -= take;

            ops.retain(head.buf_id);
            self.q.push_front(seg);
            self.total -= take;
            out.push_back(head);
            n = 0;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MockOps {
        data: HashMap<u16, Vec<u8>>,
        refs: HashMap<u16, i32>,
    }

    impl MockOps {
        fn put(&mut self, id: u16, bytes: &[u8]) {
            self.data.insert(id, bytes.to_vec());
            self.refs.insert(id, 1);
        }

        fn refs(&self, id: u16) -> i32 {
            *self.refs.get(&id).unwrap_or(&0)
        }
    }

    impl BufOps for MockOps {
        fn slice<'a>(&'a self, buf_id: u16, off: u32, len: u32) -> &'a [u8] {
            let b = self.data.get(&buf_id).expect("buffer exists");
            let s = off as usize;
            let e = s + len as usize;
            &b[s..e]
        }

        fn release(&mut self, buf_id: u16) {
            if let Some(v) = self.refs.get_mut(&buf_id) {
                *v -= 1;
            }
        }

        fn retain(&mut self, buf_id: u16) {
            let e = self.refs.entry(buf_id).or_insert(0);
            *e += 1;
        }
    }

    #[test]
    fn rx_take_chain_split_is_refcount_safe() {
        let mut ops = MockOps::default();
        ops.put(7, b"abcdef");

        let mut rx = RxQueue::new();
        rx.push_chunk(RxChunk {
            buf_id: 7,
            off: 0,
            len: 6,
        });

        let mut c = rx.take_chain(4, &mut ops).expect("take");
        assert_eq!(c.total_len(), 4);
        assert_eq!(rx.available(), 2);
        assert_eq!(ops.refs(7), 2); // split created one extra owner

        rx.consume(2, &mut ops).expect("consume tail");
        assert_eq!(ops.refs(7), 1);

        c.release(&mut ops);
        assert_eq!(ops.refs(7), 0);
    }

    #[test]
    fn chain_take_prefix_keeps_zero_copy_and_balance() {
        let mut ops = MockOps::default();
        ops.put(3, b"012345");

        let mut chain = BufChain::new();
        chain.push_back(BufSeg {
            buf_id: 3,
            off: 0,
            len: 6,
        });

        let mut head = chain.take_prefix(2, &mut ops);
        assert_eq!(head.total_len(), 2);
        assert_eq!(chain.total_len(), 4);
        assert_eq!(ops.refs(3), 2);

        head.release(&mut ops);
        assert_eq!(ops.refs(3), 1);
        chain.release(&mut ops);
        assert_eq!(ops.refs(3), 0);
    }
}
