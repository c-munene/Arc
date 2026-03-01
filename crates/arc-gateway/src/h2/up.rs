#![forbid(unsafe_code)]

use bytes::Bytes;

use arc_proto_h2::{
    error::{H2Code, H2Error},
    frame::{flags, FrameHeader, FrameType, FRAME_HEADER_LEN, PREFACE},
    hpack::{Header, HeaderBlockLimits, HpackDecoder, HpackEncoder},
    settings::{Settings, SettingsDelta},
};

use super::{
    buf::{BufChain, BufOps, RxChunk, RxQueue},
    key::ConnKey,
    tx::{Credit, TxItem},
};

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpStreamState {
    Idle,
    Open,
    HalfClosedLocal,
    HalfClosedRemote,
    Closed,
}

#[derive(Clone, Copy, Debug)]
pub struct DownLink {
    pub down: ConnKey,
    pub down_sid: u32,
}

#[derive(Debug)]
struct UpStream {
    sid: u32,
    state: UpStreamState,

    send_win: i64,
    recv_win: i64,

    down: DownLink,
}

impl UpStream {
    fn new(sid: u32, init_send: u32, init_recv: u32, down: DownLink) -> Self {
        Self {
            sid,
            state: UpStreamState::Open,
            send_win: init_send as i64,
            recv_win: init_recv as i64,
            down,
        }
    }
}

/// UpstreamSink：up->bridge 事件（DATA 仍是 BufChain 零拷贝）
pub trait UpstreamSink {
    fn on_response_headers(
        &mut self,
        up: ConnKey,
        up_sid: u32,
        end_stream: bool,
        status: u16,
        headers: Vec<Header>,
    );
    fn on_response_data(&mut self, up: ConnKey, up_sid: u32, end_stream: bool, data: BufChain);
    fn on_rst_stream(&mut self, up: ConnKey, up_sid: u32, code: H2Code);
    fn on_goaway(&mut self, up: ConnKey, last_sid: u32, code: H2Code);
    fn on_conn_error(&mut self, up: ConnKey, err: H2Error);
}

#[derive(Debug)]
struct PendingData {
    sid: u32,
    end_stream: bool,
    data: BufChain,
    credit: Option<Credit>, // 关键：不能丢，否则端到端 WINDOW_UPDATE 闭环会卡死
}

#[derive(Debug)]
struct ContState {
    sid: u32,
}

// ------------------ extremely fast u32->index map (open addressing + tombstone + rehash) ------------------

#[derive(Debug)]
struct U32Map {
    keys: Vec<u32>, // 0 empty, u32::MAX tombstone
    vals: Vec<u32>,
    mask: usize,

    live: usize,
    tomb: usize,
    seed: u64,
}

impl U32Map {
    fn with_capacity_pow2(cap: usize, seed: u64) -> Self {
        let cap = cap.max(16).next_power_of_two();
        Self {
            keys: vec![0; cap],
            vals: vec![0; cap],
            mask: cap - 1,
            live: 0,
            tomb: 0,
            seed,
        }
    }

    #[inline]
    fn hash(&self, k: u32) -> usize {
        // splitmix-ish, fast enough and stable
        let mut x = (k as u64) ^ self.seed;
        x = x.wrapping_mul(0x9e3779b97f4a7c15);
        (x as usize) & self.mask
    }

    fn should_rehash(&self) -> bool {
        let cap = self.keys.len();
        let used = self.live + self.tomb;
        used * 10 >= cap * 7 || self.tomb * 2 >= self.live.max(1)
    }

    fn rehash_to(&mut self, new_cap: usize) {
        let mut n = U32Map::with_capacity_pow2(new_cap, self.seed);
        for i in 0..self.keys.len() {
            let k = self.keys[i];
            if k != 0 && k != u32::MAX {
                let v = self.vals[i];
                n.insert_no_rehash(k, v);
            }
        }
        *self = n;
    }

    fn insert_no_rehash(&mut self, k: u32, v: u32) {
        let mut idx = self.hash(k);
        loop {
            let key = self.keys[idx];
            if key == 0 || key == u32::MAX {
                if key == u32::MAX {
                    self.tomb = self.tomb.saturating_sub(1);
                }
                self.keys[idx] = k;
                self.vals[idx] = v;
                self.live += 1;
                return;
            }
            idx = (idx + 1) & self.mask;
        }
    }

    fn get(&self, k: u32) -> Option<u32> {
        if k == 0 || k == u32::MAX {
            return None;
        }
        let mut idx = self.hash(k);
        loop {
            let key = self.keys[idx];
            if key == 0 {
                return None;
            }
            if key == k {
                return Some(self.vals[idx]);
            }
            idx = (idx + 1) & self.mask;
        }
    }

    fn insert(&mut self, k: u32, v: u32) -> bool {
        if k == 0 || k == u32::MAX {
            return false;
        }
        if self.should_rehash() {
            let cap = self.keys.len();
            let used = self.live + self.tomb;
            let grow = used * 10 >= cap * 7;
            let target = if grow { cap * 2 } else { cap };
            self.rehash_to(target);
        }

        let mut idx = self.hash(k);
        let mut first_tomb: Option<usize> = None;

        loop {
            let key = self.keys[idx];
            if key == 0 {
                let put = first_tomb.unwrap_or(idx);
                if self.keys[put] == u32::MAX {
                    self.tomb = self.tomb.saturating_sub(1);
                }
                self.keys[put] = k;
                self.vals[put] = v;
                self.live += 1;
                return true;
            }
            if key == u32::MAX {
                if first_tomb.is_none() {
                    first_tomb = Some(idx);
                }
            } else if key == k {
                self.vals[idx] = v;
                return true;
            }
            idx = (idx + 1) & self.mask;
        }
    }

    fn remove(&mut self, k: u32) -> Option<u32> {
        if k == 0 || k == u32::MAX {
            return None;
        }
        let mut idx = self.hash(k);
        loop {
            let key = self.keys[idx];
            if key == 0 {
                return None;
            }
            if key == k {
                self.keys[idx] = u32::MAX;
                self.live = self.live.saturating_sub(1);
                self.tomb += 1;
                return Some(self.vals[idx]);
            }
            idx = (idx + 1) & self.mask;
        }
    }

    fn update_value(&mut self, k: u32, v: u32) {
        if k == 0 || k == u32::MAX {
            return;
        }
        let mut idx = self.hash(k);
        loop {
            let key = self.keys[idx];
            if key == 0 {
                return;
            }
            if key == k {
                self.vals[idx] = v;
                return;
            }
            idx = (idx + 1) & self.mask;
        }
    }
}

#[derive(Debug)]
struct StreamTable {
    v: Vec<UpStream>,
    m: U32Map,
}

impl StreamTable {
    fn new(seed: u64) -> Self {
        // 预设容量：足够大但不爆
        let cap = 1024usize;
        Self {
            v: Vec::with_capacity(256),
            m: U32Map::with_capacity_pow2(cap * 2, seed),
        }
    }

    fn len(&self) -> usize {
        self.v.len()
    }

    fn get(&self, sid: u32) -> Option<&UpStream> {
        let idx = self.m.get(sid)? as usize;
        self.v.get(idx)
    }

    fn get_mut(&mut self, sid: u32) -> Option<&mut UpStream> {
        let idx = self.m.get(sid)? as usize;
        self.v.get_mut(idx)
    }

    fn insert(&mut self, s: UpStream) -> bool {
        let sid = s.sid;
        if self.m.get(sid).is_some() {
            return false;
        }
        let idx = self.v.len() as u32;
        self.v.push(s);
        self.m.insert(sid, idx)
    }

    fn remove(&mut self, sid: u32) -> Option<UpStream> {
        let idx = self.m.remove(sid)? as usize;
        let last = self.v.len() - 1;
        let removed = self.v.swap_remove(idx);
        if idx != last {
            let moved_sid = self.v[idx].sid;
            self.m.update_value(moved_sid, idx as u32);
        }
        Some(removed)
    }
}

#[derive(Debug)]
pub struct UpstreamH2 {
    pub key: ConnKey,

    rx: RxQueue,
    tx: VecDeque<TxItem>,

    // client preface already queued?
    preface_queued: bool,
    need_first_settings: bool,

    pub peer: Settings,
    pub local: Settings,

    max_frame_size_in: u32,
    max_frame_size_out: u32,

    // flow control
    send_conn_win: i64,
    recv_conn_win: i64,

    // stream ids
    next_sid: u32,
    peer_max_concurrent: u32, // 0=unknown => treat as large

    // draining/goaway
    draining: bool,
    peer_last_goaway_sid: u32,

    // streams
    streams: StreamTable,

    // HPACK
    hdec: HpackDecoder,
    henc: HpackEncoder,
    _limits: HeaderBlockLimits,

    // continuation
    cont: Option<ContState>,
    header_scratch: Vec<u8>,

    // send-side pending
    pending: VecDeque<PendingData>,
}

impl UpstreamH2 {
    pub fn new(key: ConnKey, seed: u64) -> Self {
        let mut local = Settings::default();
        local.enable_push = false;

        // 客户端强烈建议限制自己愿意并发的 streams，避免 upstream 恶意压垮
        if local.max_concurrent_streams == 0 {
            local.max_concurrent_streams = 1024;
        }

        let peer = Settings::default();

        let limits = HeaderBlockLimits::default();
        let hdec = HpackDecoder::new(limits);
        let henc = HpackEncoder::default();

        let mut s = Self {
            key,
            rx: RxQueue::new(),
            tx: VecDeque::new(),

            preface_queued: false,
            need_first_settings: true,

            peer,
            local,

            max_frame_size_in: 16_384,
            max_frame_size_out: 16_384,

            send_conn_win: 65_535,
            recv_conn_win: 65_535,

            next_sid: 1,
            peer_max_concurrent: 0,

            draining: false,
            peer_last_goaway_sid: 0,

            streams: StreamTable::new(seed),

            hdec,
            henc,
            _limits: limits,

            cont: None,
            header_scratch: Vec::with_capacity(8 * 1024),

            pending: VecDeque::new(),
        };

        s.queue_client_preface_and_settings();
        s
    }

    pub fn push_rx(&mut self, c: RxChunk) {
        self.rx.push_chunk(c);
    }
    pub fn pop_tx(&mut self) -> Option<TxItem> {
        self.tx.pop_front()
    }
    pub fn has_tx(&self) -> bool {
        !self.tx.is_empty()
    }

    /// pool 的 least-loaded 需要
    pub fn active_streams(&self) -> u32 {
        self.streams.len() as u32
    }

    pub fn is_draining(&self) -> bool {
        self.draining
    }

    pub fn mark_goaway(&mut self, last_sid: u32, _code: H2Code) {
        self.draining = true;
        self.peer_last_goaway_sid = last_sid;
    }

    fn queue_client_preface_and_settings(&mut self) {
        if self.preface_queued {
            return;
        }
        self.preface_queued = true;

        self.tx.push_back(TxItem::Raw {
            bytes: Bytes::from_static(PREFACE),
        });

        let mut payload = Vec::with_capacity(64);
        self.local.encode_into(&mut payload);

        let mut hdr = [0u8; 9];
        FrameHeader {
            len: payload.len() as u32,
            ty: FrameType::Settings,
            flags: 0,
            stream_id: 0,
        }
        .write_into(&mut hdr);
        self.tx.push_back(TxItem::FrameBytes {
            header: hdr,
            payload: Bytes::from(payload),
        });
    }

    fn queue_settings_ack(&mut self) {
        let mut hdr = [0u8; 9];
        FrameHeader {
            len: 0,
            ty: FrameType::Settings,
            flags: flags::ACK,
            stream_id: 0,
        }
        .write_into(&mut hdr);
        self.tx.push_back(TxItem::FrameBytes {
            header: hdr,
            payload: Bytes::new(),
        });
    }

    fn queue_ping_ack(&mut self, p8: [u8; 8]) {
        let mut hdr = [0u8; 9];
        FrameHeader {
            len: 8,
            ty: FrameType::Ping,
            flags: flags::ACK,
            stream_id: 0,
        }
        .write_into(&mut hdr);
        self.tx.push_back(TxItem::FrameBytes {
            header: hdr,
            payload: Bytes::copy_from_slice(&p8),
        });
    }

    fn queue_goaway(&mut self, last_sid: u32, code: H2Code) {
        let mut payload = Vec::with_capacity(8);
        payload.extend_from_slice(&(last_sid & 0x7fff_ffff).to_be_bytes());
        payload.extend_from_slice(&(code as u32).to_be_bytes());

        let mut hdr = [0u8; 9];
        FrameHeader {
            len: 8,
            ty: FrameType::Goaway,
            flags: 0,
            stream_id: 0,
        }
        .write_into(&mut hdr);

        self.tx.push_back(TxItem::FrameBytes {
            header: hdr,
            payload: Bytes::from(payload),
        });
    }

    fn queue_rst_stream(&mut self, sid: u32, code: H2Code) {
        let mut payload = [0u8; 4];
        payload.copy_from_slice(&(code as u32).to_be_bytes());

        let mut hdr = [0u8; 9];
        FrameHeader {
            len: 4,
            ty: FrameType::RstStream,
            flags: 0,
            stream_id: sid,
        }
        .write_into(&mut hdr);

        self.tx.push_back(TxItem::FrameBytes {
            header: hdr,
            payload: Bytes::copy_from_slice(&payload),
        });
    }

    fn queue_window_update(&mut self, sid: u32, delta: u32) {
        if delta == 0 {
            return;
        }
        let mut payload = [0u8; 4];
        payload.copy_from_slice(&(delta & 0x7fff_ffff).to_be_bytes());

        let mut hdr = [0u8; 9];
        FrameHeader {
            len: 4,
            ty: FrameType::WindowUpdate,
            flags: 0,
            stream_id: sid,
        }
        .write_into(&mut hdr);

        self.tx.push_back(TxItem::FrameBytes {
            header: hdr,
            payload: Bytes::copy_from_slice(&payload),
        });
    }

    pub fn send_rst_stream(&mut self, sid: u32, code: H2Code) -> Result<(), H2Error> {
        if sid == 0 {
            return Err(H2Error::new(
                H2Code::ProtocolError,
                "RST_STREAM on stream 0",
            ));
        }
        self.queue_rst_stream(sid, code);
        Ok(())
    }

    // ---------------- stream mgmt ----------------

    pub fn can_open_stream(&self) -> bool {
        if self.draining {
            return false;
        }
        if self.peer_max_concurrent == 0 {
            return true;
        }
        self.active_streams() < self.peer_max_concurrent
    }

    /// bridge 调用：为下游 stream 开一个 upstream stream
    pub fn open_stream(&mut self, down: DownLink) -> Result<u32, H2Error> {
        if !self.can_open_stream() {
            return Err(H2Error::new(
                H2Code::RefusedStream,
                "upstream max concurrent streams reached or draining",
            ));
        }

        // H2 stream id is 31-bit
        const MAX_SID: u32 = 0x7fff_ffff;

        let sid = self.next_sid;
        if sid == 0 || (sid & 1) == 0 || sid > MAX_SID {
            self.draining = true;
            return Err(H2Error::new(
                H2Code::ProtocolError,
                "upstream stream id exhausted",
            ));
        }

        let next = sid.wrapping_add(2);
        if next == 0 || next > MAX_SID {
            // 下一次就不再能开了，直接标记 draining
            self.draining = true;
        } else {
            self.next_sid = next;
        }

        let init_send = self.peer.initial_window_size;
        let init_recv = self.local.initial_window_size;

        let s = UpStream::new(sid, init_send, init_recv, down);

        if !self.streams.insert(s) {
            return Err(H2Error::new(
                H2Code::InternalError,
                "upstream stream insert failed",
            ));
        }

        Ok(sid)
    }

    pub fn downlink(&self, up_sid: u32) -> Option<DownLink> {
        self.streams.get(up_sid).map(|s| s.down)
    }

    pub fn close_stream(&mut self, up_sid: u32) {
        let _ = self.streams.remove(up_sid);
    }

    // ---------------- outbound (HEADERS/DATA + pending + flow control) ----------------

    /// 发送请求 HEADERS（HPACK 编码 + 分片）
    pub fn send_request_headers(
        &mut self,
        up_sid: u32,
        end_stream: bool,
        headers: Vec<Header>,
    ) -> Result<(), H2Error> {
        if self.streams.get(up_sid).is_none() {
            return Err(H2Error::new(
                H2Code::StreamClosed,
                "send headers on unknown upstream stream",
            ));
        }
        let block = self.henc.encode_headers(&headers);
        self.queue_headers_block(up_sid, end_stream, Bytes::from(block));
        Ok(())
    }

    fn queue_headers_block(&mut self, sid: u32, end_stream: bool, block: Bytes) {
        let max = self.max_frame_size_out as usize;

        let mut off = 0usize;
        let total = block.len();

        // HEADERS first
        let first_len = total.min(max);

        let mut hdr = [0u8; 9];
        let mut fl = 0u8;
        if end_stream {
            fl |= flags::END_STREAM;
        }
        if first_len == total {
            fl |= flags::END_HEADERS;
        }
        FrameHeader {
            len: first_len as u32,
            ty: FrameType::Headers,
            flags: fl,
            stream_id: sid,
        }
        .write_into(&mut hdr);

        self.tx.push_back(TxItem::FrameBytes {
            header: hdr,
            payload: block.slice(off..off + first_len),
        });
        off += first_len;

        // CONTINUATION
        while off < total {
            let take = (total - off).min(max);
            let mut hdr2 = [0u8; 9];
            let mut fl2 = 0u8;
            if off + take == total {
                fl2 |= flags::END_HEADERS;
            }
            FrameHeader {
                len: take as u32,
                ty: FrameType::Continuation,
                flags: fl2,
                stream_id: sid,
            }
            .write_into(&mut hdr2);

            self.tx.push_back(TxItem::FrameBytes {
                header: hdr2,
                payload: block.slice(off..off + take),
            });
            off += take;
        }
    }

    /// 发送请求 DATA（零拷贝 + send-side flow control + pending）
    pub fn send_request_data(
        &mut self,
        up_sid: u32,
        end_stream: bool,
        mut data: BufChain,
        credit: Option<Credit>,
        ops: &mut dyn BufOps,
    ) -> Result<(), H2Error> {
        if self.streams.get(up_sid).is_none() {
            data.release(ops);
            return Ok(());
        }

        let rem_credit = self.try_queue_data(up_sid, end_stream, &mut data, credit, ops)?;
        if !data.is_empty() {
            self.pending.push_back(PendingData {
                sid: up_sid,
                end_stream,
                data,
                credit: rem_credit,
            });
        }
        Ok(())
    }

    fn split_credit(
        credit: Option<Credit>,
        take: u32,
        remain: u32,
    ) -> (Option<Credit>, Option<Credit>) {
        let Some(c) = credit else {
            return (None, None);
        };
        match c {
            Credit::ToDownstream { conn, sid, bytes } => {
                let a = take.min(bytes);
                let b = bytes.saturating_sub(a);
                let c1 = if a > 0 {
                    Some(Credit::ToDownstream {
                        conn,
                        sid,
                        bytes: a,
                    })
                } else {
                    None
                };
                let c2 = if b > 0 && remain > 0 {
                    Some(Credit::ToDownstream {
                        conn,
                        sid,
                        bytes: b,
                    })
                } else {
                    None
                };
                (c1, c2)
            }
            Credit::ToUpstream { conn, sid, bytes } => {
                let a = take.min(bytes);
                let b = bytes.saturating_sub(a);
                let c1 = if a > 0 {
                    Some(Credit::ToUpstream {
                        conn,
                        sid,
                        bytes: a,
                    })
                } else {
                    None
                };
                let c2 = if b > 0 && remain > 0 {
                    Some(Credit::ToUpstream {
                        conn,
                        sid,
                        bytes: b,
                    })
                } else {
                    None
                };
                (c1, c2)
            }
        }
    }

    /// 返回：还没能入 tx 的剩余 credit（必须塞进 pending，否则背压闭环会断）
    fn try_queue_data(
        &mut self,
        sid: u32,
        end_stream: bool,
        data: &mut BufChain,
        mut credit: Option<Credit>,
        ops: &mut dyn BufOps,
    ) -> Result<Option<Credit>, H2Error> {
        loop {
            if data.is_empty() {
                return Ok(credit);
            }

            let Some(st) = self.streams.get(sid) else {
                data.release(ops);
                return Ok(None);
            };

            let st_send = st.send_win;
            let conn_send = self.send_conn_win;

            let avail = st_send.min(conn_send);
            if avail <= 0 {
                return Ok(credit);
            }

            let maxf = self.max_frame_size_out as u32;
            let want = (avail as u32).min(maxf).min(data.total_len());
            let remain_after = data.total_len().saturating_sub(want);

            let piece = data.take_prefix(want, ops);
            let is_last = data.is_empty() && end_stream;

            // consume send windows now
            self.send_conn_win -= want as i64;
            if let Some(stm) = self.streams.get_mut(sid) {
                stm.send_win -= want as i64;
            }

            let (c1, c2) = Self::split_credit(credit, want, remain_after);
            credit = c2;

            let mut hdr = [0u8; 9];
            let fl = if is_last { flags::END_STREAM } else { 0 };
            FrameHeader {
                len: want,
                ty: FrameType::Data,
                flags: fl,
                stream_id: sid,
            }
            .write_into(&mut hdr);

            self.tx.push_back(TxItem::FrameData {
                header: hdr,
                payload: piece,
                credit: c1,
            });
        }
    }

    fn flush_pending(&mut self, ops: &mut dyn BufOps) -> Result<(), H2Error> {
        let mut rounds = 0usize;

        while rounds < 1024 {
            let Some(mut p) = self.pending.pop_front() else {
                break;
            };

            let rem =
                self.try_queue_data(p.sid, p.end_stream, &mut p.data, p.credit.take(), ops)?;
            p.credit = rem;

            if !p.data.is_empty() {
                // still blocked: fairness, push back and stop
                self.pending.push_back(p);
                break;
            }

            rounds += 1;
        }

        Ok(())
    }

    /// credit：当“你已经把收到的 upstream response DATA 成功写到 downstream rustls writer 并释放 buffer”时调用
    pub fn credit_recv_window(&mut self, sid: u32, bytes: u32) {
        if bytes == 0 {
            return;
        }
        // stream
        if let Some(s) = self.streams.get_mut(sid) {
            s.recv_win += bytes as i64;
            self.queue_window_update(sid, bytes);
        }
        // conn
        self.recv_conn_win += bytes as i64;
        self.queue_window_update(0, bytes);
    }

    // ---------------- inbound parsing ----------------

    pub fn pump(&mut self, _now_ns: u64, ops: &mut dyn BufOps, sink: &mut dyn UpstreamSink) {
        let mut hdr_bytes = [0u8; FRAME_HEADER_LEN];
        let mut frames = 0usize;

        while frames < 128 {
            if !self.rx.peek_exact(FRAME_HEADER_LEN, ops, &mut hdr_bytes) {
                return;
            }

            let fh = match FrameHeader::parse(&hdr_bytes, self.max_frame_size_in) {
                Ok(x) => x,
                Err(e) => {
                    self.queue_goaway(0, e.code);
                    sink.on_conn_error(self.key, e);
                    return;
                }
            };

            if self.rx.available() < (FRAME_HEADER_LEN as u32 + fh.len) {
                return;
            }
            let _ = self.rx.consume(FRAME_HEADER_LEN as u32, ops);

            // continuation strict
            if let Some(c) = &self.cont {
                if fh.ty != FrameType::Continuation || fh.stream_id != c.sid {
                    let e = H2Error::new(H2Code::ProtocolError, "expected CONTINUATION (upstream)");
                    self.queue_goaway(0, e.code);
                    sink.on_conn_error(self.key, e);
                    return;
                }
            }

            if self.need_first_settings {
                if fh.ty != FrameType::Settings {
                    let e = H2Error::new(
                        H2Code::ProtocolError,
                        "first upstream frame must be SETTINGS",
                    );
                    self.queue_goaway(0, e.code);
                    sink.on_conn_error(self.key, e);
                    return;
                }
                self.need_first_settings = false;
            }

            match fh.ty {
                FrameType::Settings => {
                    if fh.stream_id != 0 {
                        let e = H2Error::new(H2Code::ProtocolError, "SETTINGS on stream != 0");
                        self.queue_goaway(0, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }
                    if (fh.flags & flags::ACK) != 0 {
                        if fh.len != 0 {
                            let e =
                                H2Error::new(H2Code::FrameSizeError, "SETTINGS ack non-zero len");
                            self.queue_goaway(0, e.code);
                            sink.on_conn_error(self.key, e);
                            return;
                        }
                    } else {
                        self.header_scratch.clear();
                        let _ = self.rx.copy_into_vec(fh.len, ops, &mut self.header_scratch);

                        let delta: SettingsDelta =
                            match self.peer.apply_from_payload(&self.header_scratch) {
                                Ok(d) => d,
                                Err(e) => {
                                    self.queue_goaway(0, e.code);
                                    sink.on_conn_error(self.key, e);
                                    return;
                                }
                            };

                        self.max_frame_size_in = self.peer.max_frame_size;
                        self.max_frame_size_out = self.peer.max_frame_size;

                        // peer max concurrent
                        self.peer_max_concurrent = self.peer.max_concurrent_streams;

                        // apply initial_window delta to existing streams send_win
                        let d = delta.initial_window_delta();
                        if d != 0 {
                            for s in self.streams.v.iter_mut() {
                                s.send_win += d;
                                if s.send_win > 0x7fff_ffff {
                                    let e = H2Error::new(
                                        H2Code::FlowControlError,
                                        "send window overflow",
                                    );
                                    self.queue_goaway(0, e.code);
                                    sink.on_conn_error(self.key, e);
                                    return;
                                }
                            }
                            // window changed, try flush pending
                            let _ = self.flush_pending(ops);
                        }

                        self.queue_settings_ack();
                    }
                }

                FrameType::Ping => {
                    if fh.stream_id != 0 || fh.len != 8 {
                        let e = H2Error::new(H2Code::ProtocolError, "bad PING");
                        self.queue_goaway(0, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }
                    self.header_scratch.clear();
                    let _ = self.rx.copy_into_vec(8, ops, &mut self.header_scratch);
                    let mut p8 = [0u8; 8];
                    p8.copy_from_slice(&self.header_scratch[..8]);
                    if (fh.flags & flags::ACK) == 0 {
                        self.queue_ping_ack(p8);
                    }
                }

                FrameType::WindowUpdate => {
                    if fh.len != 4 {
                        let e = H2Error::new(H2Code::FrameSizeError, "WINDOW_UPDATE len != 4");
                        self.queue_goaway(0, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }
                    self.header_scratch.clear();
                    let _ = self.rx.copy_into_vec(4, ops, &mut self.header_scratch);
                    let mut v = u32::from_be_bytes([
                        self.header_scratch[0],
                        self.header_scratch[1],
                        self.header_scratch[2],
                        self.header_scratch[3],
                    ]);
                    v &= 0x7fff_ffff;
                    if v == 0 {
                        let e = H2Error::new(H2Code::ProtocolError, "WINDOW_UPDATE increment 0");
                        self.queue_goaway(0, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }

                    if fh.stream_id == 0 {
                        self.send_conn_win += v as i64;
                        if self.send_conn_win > 0x7fff_ffff {
                            let e =
                                H2Error::new(H2Code::FlowControlError, "conn send window overflow");
                            self.queue_goaway(0, e.code);
                            sink.on_conn_error(self.key, e);
                            return;
                        }
                        let _ = self.flush_pending(ops);
                    } else if let Some(s) = self.streams.get_mut(fh.stream_id) {
                        s.send_win += v as i64;
                        if s.send_win > 0x7fff_ffff {
                            let e = H2Error::new(
                                H2Code::FlowControlError,
                                "stream send window overflow",
                            );
                            self.queue_goaway(0, e.code);
                            sink.on_conn_error(self.key, e);
                            return;
                        }
                        let _ = self.flush_pending(ops);
                    }
                }

                FrameType::RstStream => {
                    if fh.stream_id == 0 || fh.len != 4 {
                        let e = H2Error::new(H2Code::ProtocolError, "bad RST_STREAM");
                        self.queue_goaway(0, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }
                    self.header_scratch.clear();
                    let _ = self.rx.copy_into_vec(4, ops, &mut self.header_scratch);
                    let code = H2Code::from_u32(u32::from_be_bytes([
                        self.header_scratch[0],
                        self.header_scratch[1],
                        self.header_scratch[2],
                        self.header_scratch[3],
                    ]));

                    self.close_stream(fh.stream_id);
                    sink.on_rst_stream(self.key, fh.stream_id, code);
                }

                FrameType::Goaway => {
                    if fh.stream_id != 0 || fh.len < 8 {
                        let e = H2Error::new(H2Code::ProtocolError, "bad GOAWAY");
                        self.queue_goaway(0, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }
                    self.header_scratch.clear();
                    let _ = self.rx.copy_into_vec(fh.len, ops, &mut self.header_scratch);

                    let last = u32::from_be_bytes([
                        self.header_scratch[0],
                        self.header_scratch[1],
                        self.header_scratch[2],
                        self.header_scratch[3],
                    ]) & 0x7fff_ffff;
                    let code = H2Code::from_u32(u32::from_be_bytes([
                        self.header_scratch[4],
                        self.header_scratch[5],
                        self.header_scratch[6],
                        self.header_scratch[7],
                    ]));

                    self.mark_goaway(last, code);
                    sink.on_goaway(self.key, last, code);
                }

                FrameType::Headers => {
                    let sid = fh.stream_id;
                    if sid == 0 {
                        let e = H2Error::new(H2Code::ProtocolError, "HEADERS on stream 0");
                        self.queue_goaway(0, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }

                    let mut pad_len: u8 = 0;
                    if (fh.flags & flags::PADDED) != 0 {
                        pad_len = match self.rx.read_u8(ops) {
                            Ok(x) => x,
                            Err(_) => return,
                        };
                    }
                    if (fh.flags & flags::PRIORITY) != 0 {
                        let _ = self.rx.consume(5, ops);
                    }

                    let mut frag_len = fh.len as i64;
                    if (fh.flags & flags::PADDED) != 0 {
                        frag_len -= 1;
                    }
                    if (fh.flags & flags::PRIORITY) != 0 {
                        frag_len -= 5;
                    }
                    frag_len -= pad_len as i64;
                    if frag_len < 0 {
                        let e = H2Error::new(H2Code::ProtocolError, "bad HEADERS length");
                        self.queue_goaway(0, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }

                    self.header_scratch.clear();
                    if self
                        .rx
                        .copy_into_vec(frag_len as u32, ops, &mut self.header_scratch)
                        .is_err()
                    {
                        return;
                    }
                    if pad_len > 0 {
                        let _ = self.rx.consume(pad_len as u32, ops);
                    }

                    if (fh.flags & flags::END_HEADERS) == 0 {
                        self.cont = Some(ContState { sid });
                    } else {
                        let decoded = match self.hdec.decode(&self.header_scratch) {
                            Ok(h) => h,
                            Err(e) => {
                                self.queue_goaway(0, e.code);
                                sink.on_conn_error(self.key, e);
                                return;
                            }
                        };

                        let (status, hdrs) = match parse_response_headers(decoded) {
                            Ok(x) => x,
                            Err(e) => {
                                self.queue_rst_stream(sid, e.code);
                                sink.on_conn_error(self.key, e);
                                return;
                            }
                        };

                        let end_stream = (fh.flags & flags::END_STREAM) != 0;
                        if end_stream {
                            if let Some(s) = self.streams.get_mut(sid) {
                                s.state = UpStreamState::HalfClosedRemote;
                            }
                        }

                        sink.on_response_headers(self.key, sid, end_stream, status, hdrs);
                    }
                }

                FrameType::Continuation => {
                    let sid = fh.stream_id;
                    if self.cont.as_ref().map(|c| c.sid) != Some(sid) {
                        let e = H2Error::new(H2Code::ProtocolError, "unexpected CONTINUATION");
                        self.queue_goaway(0, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }
                    if self
                        .rx
                        .copy_into_vec(fh.len, ops, &mut self.header_scratch)
                        .is_err()
                    {
                        return;
                    }
                    if (fh.flags & flags::END_HEADERS) != 0 {
                        self.cont = None;

                        let decoded = match self.hdec.decode(&self.header_scratch) {
                            Ok(h) => h,
                            Err(e) => {
                                self.queue_goaway(0, e.code);
                                sink.on_conn_error(self.key, e);
                                return;
                            }
                        };

                        let (status, hdrs) = match parse_response_headers(decoded) {
                            Ok(x) => x,
                            Err(e) => {
                                self.queue_rst_stream(sid, e.code);
                                sink.on_conn_error(self.key, e);
                                return;
                            }
                        };

                        sink.on_response_headers(self.key, sid, false, status, hdrs);
                    }
                }

                FrameType::Data => {
                    let sid = fh.stream_id;
                    if sid == 0 {
                        let e = H2Error::new(H2Code::ProtocolError, "DATA on stream 0");
                        self.queue_goaway(0, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }
                    if self.streams.get(sid).is_none() {
                        let e = H2Error::new(H2Code::StreamClosed, "DATA on closed stream");
                        self.queue_goaway(0, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }

                    let end_stream = (fh.flags & flags::END_STREAM) != 0;

                    if (fh.flags & flags::PADDED) != 0 {
                        let pad_len = match self.rx.read_u8(ops) {
                            Ok(x) => x as u32,
                            Err(_) => return,
                        };
                        if pad_len + 1 > fh.len {
                            let e = H2Error::new(H2Code::ProtocolError, "bad DATA padding");
                            self.queue_goaway(0, e.code);
                            sink.on_conn_error(self.key, e);
                            return;
                        }
                        let data_len = fh.len - 1 - pad_len;
                        let mut data = match self.rx.take_chain(data_len, ops) {
                            Ok(c) => c,
                            Err(_) => return,
                        };
                        if pad_len > 0 {
                            let _ = self.rx.consume(pad_len, ops);
                        }

                        self.recv_conn_win -= data_len as i64;
                        if self.recv_conn_win < 0 {
                            let e = H2Error::new(
                                H2Code::FlowControlError,
                                "up conn recv window negative",
                            );
                            self.queue_goaway(0, e.code);
                            sink.on_conn_error(self.key, e);
                            data.release(ops);
                            return;
                        }
                        if let Some(s) = self.streams.get_mut(sid) {
                            s.recv_win -= data_len as i64;
                            if s.recv_win < 0 {
                                self.queue_rst_stream(sid, H2Code::FlowControlError);
                                data.release(ops);
                                return;
                            }
                            if end_stream {
                                s.state = UpStreamState::HalfClosedRemote;
                            }
                        }

                        sink.on_response_data(self.key, sid, end_stream, data);
                    } else {
                        let data_len = fh.len;
                        let mut data = match self.rx.take_chain(data_len, ops) {
                            Ok(c) => c,
                            Err(_) => return,
                        };

                        self.recv_conn_win -= data_len as i64;
                        if self.recv_conn_win < 0 {
                            let e = H2Error::new(
                                H2Code::FlowControlError,
                                "up conn recv window negative",
                            );
                            self.queue_goaway(0, e.code);
                            sink.on_conn_error(self.key, e);
                            data.release(ops);
                            return;
                        }
                        if let Some(s) = self.streams.get_mut(sid) {
                            s.recv_win -= data_len as i64;
                            if s.recv_win < 0 {
                                self.queue_rst_stream(sid, H2Code::FlowControlError);
                                data.release(ops);
                                return;
                            }
                            if end_stream {
                                s.state = UpStreamState::HalfClosedRemote;
                            }
                        }

                        sink.on_response_data(self.key, sid, end_stream, data);
                    }
                }

                _ => {
                    let _ = self.rx.consume(fh.len, ops);
                }
            }

            frames += 1;
        }
    }
}

fn parse_response_headers(mut headers: Vec<Header>) -> Result<(u16, Vec<Header>), H2Error> {
    // must have :status
    let mut status: Option<u16> = None;
    let mut out: Vec<Header> = Vec::with_capacity(headers.len());

    let mut saw_regular = false;
    for h in headers.drain(..) {
        if h.name.starts_with(b":") {
            if saw_regular {
                return Err(H2Error::new(
                    H2Code::ProtocolError,
                    "pseudo header after regular header (resp)",
                ));
            }
            if h.name.as_ref() == b":status" {
                let v = h.value;
                if v.len() != 3 {
                    return Err(H2Error::new(H2Code::ProtocolError, "bad :status"));
                }
                // digit validation
                if !(v[0].is_ascii_digit() && v[1].is_ascii_digit() && v[2].is_ascii_digit()) {
                    return Err(H2Error::new(H2Code::ProtocolError, "bad :status digits"));
                }
                let d0 = (v[0] - b'0') as u16;
                let d1 = (v[1] - b'0') as u16;
                let d2 = (v[2] - b'0') as u16;
                status = Some(d0 * 100 + d1 * 10 + d2);
            } else {
                return Err(H2Error::new(
                    H2Code::ProtocolError,
                    "unknown pseudo header in response",
                ));
            }
        } else {
            saw_regular = true;
            out.push(h);
        }
    }

    let st = status.ok_or_else(|| H2Error::new(H2Code::ProtocolError, "missing :status"))?;
    Ok((st, out))
}
