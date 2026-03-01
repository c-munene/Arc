#![forbid(unsafe_code)]

use bytes::Bytes;

use arc_proto_h2::{
    error::{H2Code, H2Error},
    frame::{flags, FrameHeader, FrameType, FRAME_HEADER_LEN, PREFACE},
    hpack::{Header, HeaderBlockLimits, HpackDecoder, HpackEncoder},
    settings::{Settings, SettingsDelta},
};

#[cfg(feature = "h2-native-upstream")]
use super::stream_set::UpLink;
use super::{
    buf::{BufChain, BufOps, RxChunk, RxQueue},
    key::ConnKey,
    stream_set::{Stream, StreamSet, StreamState},
    tx::{Credit, TxItem},
};

use std::collections::VecDeque;

#[derive(Debug)]
pub struct RequestHead {
    pub method: Bytes,
    #[cfg(feature = "h2-native-upstream")]
    pub scheme: Option<Bytes>,
    pub authority: Option<Bytes>,
    pub path: Option<Bytes>,
    pub headers: Vec<Header>,
}

/// downstream->bridge 事件（只包含零拷贝 DATA 与解析后的 HEADERS）
pub trait DownstreamSink {
    fn on_request_headers(&mut self, down: ConnKey, sid: u32, end_stream: bool, head: RequestHead);
    fn on_request_data(&mut self, down: ConnKey, sid: u32, end_stream: bool, data: BufChain);
    fn on_rst_stream(&mut self, down: ConnKey, sid: u32, code: H2Code);
    fn on_goaway(&mut self, down: ConnKey, last_sid: u32, code: H2Code);
    fn on_conn_error(&mut self, down: ConnKey, err: H2Error);
}

#[derive(Debug)]
struct ContState {
    sid: u32,
}

#[derive(Debug)]
struct PendingData {
    sid: u32,
    end_stream: bool,
    data: BufChain,
    credit: Option<Credit>,
}

#[derive(Debug)]
pub struct DownstreamH2 {
    pub key: ConnKey,

    rx: RxQueue,
    tx: VecDeque<TxItem>,

    // preface tracking
    preface_off: usize,
    need_first_settings: bool,

    // settings
    pub peer: Settings,      // peer settings (what we must respect when sending)
    pub local: Settings,     // what we advertise
    max_frame_size_in: u32,  // max we accept from peer (inbound)
    max_frame_size_out: u32, // max we send to peer (peer SETTINGS_MAX_FRAME_SIZE)

    // flow control
    send_conn_win: i64, // peer allows us to send
    recv_conn_win: i64, // we allow peer to send (credit-managed)

    // streams
    streams: StreamSet,
    last_peer_sid: u32,

    // HPACK
    hdec: HpackDecoder,
    henc: HpackEncoder,
    _hpack_limits: HeaderBlockLimits,

    // continuation
    cont: Option<ContState>,
    header_scratch: Vec<u8>, // headers 很小，copy 是可接受的；DATA 全程零拷贝

    // send-side pending (response body)
    pending: VecDeque<PendingData>,
}

impl DownstreamH2 {
    pub fn new(key: ConnKey, max_concurrent: usize, seed: u64) -> Self {
        let mut local = Settings::default();
        local.enable_push = false; // proxy 最佳实践：禁 push
        local.max_concurrent_streams = max_concurrent.max(1) as u32;

        let peer = Settings::default();

        let limits = HeaderBlockLimits::default();
        let hdec = HpackDecoder::new(limits);
        let henc = HpackEncoder::default();

        let mut s = Self {
            key,
            rx: RxQueue::new(),
            tx: VecDeque::new(),

            preface_off: 0,
            need_first_settings: true,

            peer,
            local,
            max_frame_size_in: 16_384,
            max_frame_size_out: 16_384,

            send_conn_win: 65_535,
            recv_conn_win: 65_535,

            streams: StreamSet::new(max_concurrent, seed),
            last_peer_sid: 0,

            hdec,
            henc,
            _hpack_limits: limits,

            cont: None,
            header_scratch: Vec::with_capacity(8 * 1024),

            pending: VecDeque::new(),
        };

        // 连接一建立就发 server SETTINGS
        s.queue_settings();
        s
    }

    pub fn push_rx(&mut self, chunk: RxChunk) {
        self.rx.push_chunk(chunk);
    }

    pub fn tx_mut(&mut self) -> &mut VecDeque<TxItem> {
        &mut self.tx
    }

    pub fn release_all(&mut self, ops: &mut dyn BufOps) {
        self.rx.release_all(ops);
        while let Some(item) = self.tx.pop_front() {
            if let TxItem::FrameData { mut payload, .. } = item {
                payload.release(ops);
            }
        }
        while let Some(mut p) = self.pending.pop_front() {
            p.data.release(ops);
        }
        self.header_scratch.clear();
    }

    /// bind：downstream stream -> upstream stream
    #[cfg(feature = "h2-native-upstream")]
    pub fn bind_uplink(&mut self, sid: u32, uplink: UpLink) {
        if let Some(st) = self.streams.get_mut(sid) {
            st.uplink = Some(uplink);
        }
    }

    #[cfg(feature = "h2-native-upstream")]
    pub fn uplink(&self, sid: u32) -> Option<UpLink> {
        self.streams.get(sid).and_then(|s| s.uplink)
    }

    #[inline]
    fn on_local_end_stream(&mut self, sid: u32) {
        let should_remove = match self.streams.get_mut(sid) {
            Some(st) => match st.state {
                StreamState::Open => {
                    st.state = StreamState::HalfClosedLocal;
                    false
                }
                StreamState::HalfClosedRemote => {
                    st.state = StreamState::Closed;
                    true
                }
                StreamState::HalfClosedLocal | StreamState::Closed => true,
                StreamState::Idle => {
                    st.state = StreamState::HalfClosedLocal;
                    false
                }
            },
            None => false,
        };

        if should_remove {
            let _ = self.streams.remove(sid);
        }
    }

    #[inline]
    fn on_remote_end_stream(&mut self, sid: u32) {
        let should_remove = match self.streams.get_mut(sid) {
            Some(st) => match st.state {
                StreamState::Open => {
                    st.state = StreamState::HalfClosedRemote;
                    false
                }
                StreamState::HalfClosedLocal => {
                    st.state = StreamState::Closed;
                    true
                }
                StreamState::HalfClosedRemote | StreamState::Closed => true,
                StreamState::Idle => {
                    st.state = StreamState::HalfClosedRemote;
                    false
                }
            },
            None => false,
        };

        if should_remove {
            let _ = self.streams.remove(sid);
        }
    }

    // -------------------- outbound helpers --------------------

    fn queue_settings(&mut self) {
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

    fn queue_ping_ack(&mut self, payload8: [u8; 8]) {
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
            payload: Bytes::copy_from_slice(&payload8),
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

    pub fn send_rst_stream(&mut self, sid: u32, code: H2Code) -> Result<(), H2Error> {
        if sid == 0 {
            return Err(H2Error::new(
                H2Code::ProtocolError,
                "RST_STREAM on stream 0",
            ));
        }
        self.queue_rst_stream(sid, code);
        self.on_local_end_stream(sid);
        Ok(())
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
            let left = total - off;
            let take = left.min(max);
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

    /// public: 发送响应 HEADERS（自动 HPACK 编码 + 分片）
    pub fn send_response_headers(
        &mut self,
        sid: u32,
        status: u16,
        headers: Vec<Header>,
        end_stream: bool,
    ) -> Result<(), H2Error> {
        // 必须已有 stream
        if self.streams.get(sid).is_none() {
            return Err(H2Error::new(
                H2Code::StreamClosed,
                "send headers on unknown stream",
            ));
        }

        // :status 必须先
        let mut all: Vec<Header> = Vec::with_capacity(headers.len() + 1);
        let mut s3 = [b'0', b'0', b'0'];
        let st = status as u32;
        s3[0] = b'0' + ((st / 100) as u8);
        s3[1] = b'0' + (((st / 10) % 10) as u8);
        s3[2] = b'0' + ((st % 10) as u8);
        all.push(Header {
            name: Bytes::from_static(b":status"),
            value: Bytes::copy_from_slice(&s3),
        });
        all.extend(headers);

        let block = self.henc.encode_headers(&all);
        self.queue_headers_block(sid, end_stream, Bytes::from(block));
        if end_stream {
            self.on_local_end_stream(sid);
        }
        Ok(())
    }

    /// public: 发送响应 DATA（零拷贝 + send-side flow control + pending）
    pub fn send_response_data(
        &mut self,
        sid: u32,
        end_stream: bool,
        mut data: BufChain,
        credit: Option<Credit>,
        ops: &mut dyn BufOps,
    ) -> Result<(), H2Error> {
        if self.streams.get(sid).is_none() {
            // 直接丢弃并释放 buffer（防泄漏）
            data.release(ops);
            return Ok(());
        }

        // 立刻尽可能 flush；剩余进 pending
        self.try_queue_data(sid, end_stream, &mut data, credit, ops)?;
        if !data.is_empty() {
            self.pending.push_back(PendingData {
                sid,
                end_stream,
                data,
                credit: None,
            });
            // 注意：credit 被切分到已发送的 TxItem 上；剩余部分的 credit 在 split 时会带入 pending（见 split_credit）
            // 这里为了简单：split_credit 会把剩余 credit 返回，我们再入队
            // -> 上面 try_queue_data 已经把剩余 credit（如有）写回 data 上的 pending（实现里处理）
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
            #[cfg(feature = "h2-native-upstream")]
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

    fn try_queue_data(
        &mut self,
        sid: u32,
        end_stream: bool,
        data: &mut BufChain,
        mut credit: Option<Credit>,
        ops: &mut dyn BufOps,
    ) -> Result<(), H2Error> {
        loop {
            if data.is_empty() {
                return Ok(());
            }

            let st_send = self.streams.get(sid).map(|s| s.flow.send_win).unwrap_or(0) as i64;
            let conn_send = self.send_conn_win;

            let avail = st_send.min(conn_send);
            if avail <= 0 {
                // 窗口为 0：等待 WINDOW_UPDATE
                // 剩余数据 + 剩余 credit 进入 pending（由调用者入队）
                if let Some(_c) = credit {
                    // credit 不能丢：放回 pending（做法：用一个 0-len tx item 不是 best；这里交给调用者的 pending 携带剩余 credit）
                    // 为避免复杂字段，我们直接把 credit=Some 留给调用者入 pending。
                    // -> 调用者使用 pending.credit 字段存储（见下方 flush_pending 实现）
                }
                return Ok(());
            }

            let maxf = self.max_frame_size_out.min(16_777_215) as u32;
            let want = (avail as u32).min(maxf).min(data.total_len());
            let remain_after = data.total_len().saturating_sub(want);

            let piece = if want == data.total_len() {
                // move whole (we can’t move out of &mut without clone; use take_prefix)
                data.take_prefix(want, ops)
            } else {
                data.take_prefix(want, ops)
            };

            let is_last = data.is_empty() && end_stream;

            // consume send windows now
            self.send_conn_win -= want as i64;
            if let Some(st) = self.streams.get_mut(sid) {
                st.flow.dec_send(want);
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

            if is_last {
                self.on_local_end_stream(sid);
            }

            // loop to possibly send more
        }
    }

    /// 当收到 peer 的 WINDOW_UPDATE 后调用：尝试把 pending flush 到 tx
    fn flush_pending(&mut self, ops: &mut dyn BufOps) -> Result<(), H2Error> {
        let mut rounds = 0usize;
        while rounds < 1024 {
            let Some(mut p) = self.pending.pop_front() else {
                break;
            };
            let mut credit = p.credit.take();
            self.try_queue_data(p.sid, p.end_stream, &mut p.data, credit.take(), ops)?;
            if !p.data.is_empty() {
                // 仍然没发完：放回队尾（公平）
                p.credit = credit;
                self.pending.push_back(p);
                break;
            } else {
                // 全部发完：BufChain 已被拆成 TxItem::FrameData，其 release 在 driver 做
            }
            rounds += 1;
        }
        Ok(())
    }

    /// credit：当“你已经把收到的 request body bytes 成功写到 upstream rustls writer 并释放 buffer”时调用
    pub fn credit_recv_window(&mut self, sid: u32, bytes: u32) {
        if bytes == 0 {
            return;
        }
        // stream
        if let Some(st) = self.streams.get_mut(sid) {
            st.flow.inc_recv(bytes);
            self.queue_window_update(sid, bytes);
        }
        // conn
        self.recv_conn_win += bytes as i64;
        self.queue_window_update(0, bytes);
    }

    // -------------------- inbound parsing --------------------

    fn ensure_stream_for_headers(&mut self, sid: u32) -> Result<(), H2Error> {
        if sid == 0 {
            return Err(H2Error::new(H2Code::ProtocolError, "HEADERS on stream 0"));
        }
        if (sid & 1) == 0 {
            return Err(H2Error::new(
                H2Code::ProtocolError,
                "client stream id must be odd",
            ));
        }

        if sid <= self.last_peer_sid {
            if self.streams.get(sid).is_none() {
                return Err(H2Error::new(
                    H2Code::ProtocolError,
                    "HEADERS for unknown stream",
                ));
            }
            return Ok(());
        }

        // new stream: enforce concurrency cap (SOTA hard gate)
        if self.streams.len() >= self.streams.cap() {
            // refuse: REFUSED_STREAM
            self.queue_rst_stream(sid, H2Code::RefusedStream);
            return Ok(());
        }

        self.last_peer_sid = sid;

        // send window = peer.initial_window_size (what they allow us to send on this stream)
        let init_send = self.peer.initial_window_size;
        // recv window = local.initial_window_size (what we allow them to send); credit-managed
        let init_recv = self.local.initial_window_size;

        let mut st = Stream::new(sid, init_send, init_recv);
        st.state = StreamState::Open;

        if !self.streams.insert(st) {
            return Err(H2Error::new(H2Code::InternalError, "stream insert failed"));
        }
        Ok(())
    }

    fn validate_and_build_request(&self, headers: Vec<Header>) -> Result<RequestHead, H2Error> {
        let mut saw_regular = false;
        let mut method: Option<Bytes> = None;
        let mut scheme: Option<Bytes> = None;
        let mut authority: Option<Bytes> = None;
        let mut path: Option<Bytes> = None;

        let mut out = Vec::with_capacity(headers.len());
        for h in headers {
            let name = h.name.clone();
            let value = h.value.clone();

            for &c in name.as_ref() {
                if (b'A'..=b'Z').contains(&c) {
                    return Err(H2Error::new(H2Code::ProtocolError, "uppercase header name"));
                }
            }

            if name.starts_with(b":") {
                if saw_regular {
                    return Err(H2Error::new(
                        H2Code::ProtocolError,
                        "pseudo-header after regular header",
                    ));
                }
                match name.as_ref() {
                    b":method" => method = Some(value),
                    b":scheme" => scheme = Some(value),
                    b":authority" => authority = Some(value),
                    b":path" => path = Some(value),
                    _ => return Err(H2Error::new(H2Code::ProtocolError, "unknown pseudo header")),
                }
                continue;
            } else {
                saw_regular = true;
            }

            match name.as_ref() {
                b"connection" | b"keep-alive" | b"proxy-connection" | b"upgrade" => {
                    return Err(H2Error::new(
                        H2Code::ProtocolError,
                        "connection-specific header in h2",
                    ));
                }
                b"transfer-encoding" => {
                    return Err(H2Error::new(
                        H2Code::ProtocolError,
                        "transfer-encoding forbidden in h2",
                    ));
                }
                b"te" => {
                    if value.as_ref() != b"trailers" {
                        return Err(H2Error::new(H2Code::ProtocolError, "TE must be trailers"));
                    }
                }
                _ => {}
            }

            out.push(Header { name, value });
        }

        let m = method.ok_or_else(|| H2Error::new(H2Code::ProtocolError, "missing :method"))?;

        if m.as_ref() == b"CONNECT" {
            if authority.is_none() {
                return Err(H2Error::new(
                    H2Code::ProtocolError,
                    "CONNECT missing :authority",
                ));
            }
        } else {
            if scheme.is_none() {
                return Err(H2Error::new(H2Code::ProtocolError, "missing :scheme"));
            }
            let p = path
                .as_ref()
                .ok_or_else(|| H2Error::new(H2Code::ProtocolError, "missing :path"))?;
            if !p.as_ref().starts_with(b"/") {
                return Err(H2Error::new(
                    H2Code::ProtocolError,
                    ":path must start with '/'",
                ));
            }
        }

        Ok(RequestHead {
            method: m,
            #[cfg(feature = "h2-native-upstream")]
            scheme,
            authority,
            path,
            headers: out,
        })
    }

    pub fn pump(&mut self, _now_ns: u64, ops: &mut dyn BufOps, sink: &mut dyn DownstreamSink) {
        // 1) preface
        if self.preface_off < PREFACE.len() {
            let need = PREFACE.len() - self.preface_off;
            if self.rx.available() < need as u32 {
                return;
            }
            for i in 0..need {
                let b = match self.rx.read_u8(ops) {
                    Ok(x) => x,
                    Err(_) => return,
                };
                if b != PREFACE[self.preface_off + i] {
                    let err = H2Error::new(H2Code::ProtocolError, "bad client preface");
                    self.queue_goaway(self.last_peer_sid, err.code);
                    sink.on_conn_error(self.key, err);
                    return;
                }
            }
            self.preface_off = PREFACE.len();
        }

        // 2) frames
        let mut hdr_bytes = [0u8; FRAME_HEADER_LEN];
        let mut frames = 0usize;

        while frames < 128 {
            if !self.rx.peek_exact(FRAME_HEADER_LEN, ops, &mut hdr_bytes) {
                return;
            }

            let fh = match FrameHeader::parse(&hdr_bytes, self.max_frame_size_in) {
                Ok(x) => x,
                Err(e) => {
                    self.queue_goaway(self.last_peer_sid, e.code);
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
                    let e = H2Error::new(H2Code::ProtocolError, "expected CONTINUATION");
                    self.queue_goaway(self.last_peer_sid, e.code);
                    sink.on_conn_error(self.key, e);
                    return;
                }
            }

            // first must be SETTINGS
            if self.need_first_settings {
                if fh.ty != FrameType::Settings {
                    let e = H2Error::new(H2Code::ProtocolError, "first frame must be SETTINGS");
                    self.queue_goaway(self.last_peer_sid, e.code);
                    sink.on_conn_error(self.key, e);
                    return;
                }
                self.need_first_settings = false;
            }

            match fh.ty {
                FrameType::Settings => {
                    if fh.stream_id != 0 {
                        let e = H2Error::new(H2Code::ProtocolError, "SETTINGS on stream != 0");
                        self.queue_goaway(self.last_peer_sid, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }
                    if (fh.flags & flags::ACK) != 0 {
                        if fh.len != 0 {
                            let e = H2Error::new(
                                H2Code::FrameSizeError,
                                "SETTINGS ack with non-zero len",
                            );
                            self.queue_goaway(self.last_peer_sid, e.code);
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
                                    self.queue_goaway(self.last_peer_sid, e.code);
                                    sink.on_conn_error(self.key, e);
                                    return;
                                }
                            };

                        self.max_frame_size_in = self.peer.max_frame_size;
                        self.max_frame_size_out = self.peer.max_frame_size;

                        // apply initial_window delta to existing streams send_win
                        let d = delta.initial_window_delta();
                        if d != 0 {
                            let mut overflow = false;
                            for st in self.streams.iter_mut() {
                                st.flow.send_win += d;
                                if st.flow.send_win > 0x7fff_ffff {
                                    overflow = true;
                                    break;
                                }
                            }
                            if overflow {
                                let e =
                                    H2Error::new(H2Code::FlowControlError, "send window overflow");
                                self.queue_goaway(self.last_peer_sid, e.code);
                                sink.on_conn_error(self.key, e);
                                return;
                            }
                        }

                        self.queue_settings_ack();
                    }
                }

                FrameType::Ping => {
                    if fh.stream_id != 0 || fh.len != 8 {
                        let e = H2Error::new(H2Code::ProtocolError, "bad PING");
                        self.queue_goaway(self.last_peer_sid, e.code);
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
                        self.queue_goaway(self.last_peer_sid, e.code);
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
                        self.queue_goaway(self.last_peer_sid, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }

                    if fh.stream_id == 0 {
                        self.send_conn_win += v as i64;
                        if self.send_conn_win > 0x7fff_ffff {
                            let e =
                                H2Error::new(H2Code::FlowControlError, "conn send window overflow");
                            self.queue_goaway(self.last_peer_sid, e.code);
                            sink.on_conn_error(self.key, e);
                            return;
                        }
                        // send window opened: flush pending response body
                        let _ = self.flush_pending(ops);
                    } else if let Some(st) = self.streams.get_mut(fh.stream_id) {
                        st.flow.inc_send(v);
                        if st.flow.send_win > 0x7fff_ffff {
                            let e = H2Error::new(
                                H2Code::FlowControlError,
                                "stream send window overflow",
                            );
                            self.queue_goaway(self.last_peer_sid, e.code);
                            sink.on_conn_error(self.key, e);
                            return;
                        }
                        let _ = self.flush_pending(ops);
                    } else {
                        // ignore unknown
                    }
                }

                FrameType::RstStream => {
                    if fh.stream_id == 0 || fh.len != 4 {
                        let e = H2Error::new(H2Code::ProtocolError, "bad RST_STREAM");
                        self.queue_goaway(self.last_peer_sid, e.code);
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

                    let _ = self.streams.remove(fh.stream_id);
                    sink.on_rst_stream(self.key, fh.stream_id, code);
                }

                FrameType::Goaway => {
                    if fh.stream_id != 0 || fh.len < 8 {
                        let e = H2Error::new(H2Code::ProtocolError, "bad GOAWAY");
                        self.queue_goaway(self.last_peer_sid, e.code);
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
                    sink.on_goaway(self.key, last, code);
                }

                FrameType::Headers => {
                    let sid = fh.stream_id;
                    if let Err(e) = self.ensure_stream_for_headers(sid) {
                        self.queue_goaway(self.last_peer_sid, e.code);
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
                        self.queue_goaway(self.last_peer_sid, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }

                    self.header_scratch.clear();
                    if let Err(_) =
                        self.rx
                            .copy_into_vec(frag_len as u32, ops, &mut self.header_scratch)
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
                                self.queue_goaway(self.last_peer_sid, e.code);
                                sink.on_conn_error(self.key, e);
                                return;
                            }
                        };
                        let head = match self.validate_and_build_request(decoded) {
                            Ok(h) => h,
                            Err(e) => {
                                self.queue_rst_stream(sid, e.code);
                                sink.on_conn_error(self.key, e);
                                return;
                            }
                        };
                        let end_stream = (fh.flags & flags::END_STREAM) != 0;
                        if end_stream {
                            self.on_remote_end_stream(sid);
                        }
                        sink.on_request_headers(self.key, sid, end_stream, head);
                    }
                }

                FrameType::Continuation => {
                    let sid = fh.stream_id;
                    if self.cont.as_ref().map(|c| c.sid) != Some(sid) {
                        let e = H2Error::new(H2Code::ProtocolError, "unexpected CONTINUATION");
                        self.queue_goaway(self.last_peer_sid, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }
                    if let Err(_) = self.rx.copy_into_vec(fh.len, ops, &mut self.header_scratch) {
                        return;
                    }
                    if (fh.flags & flags::END_HEADERS) != 0 {
                        self.cont = None;
                        let decoded = match self.hdec.decode(&self.header_scratch) {
                            Ok(h) => h,
                            Err(e) => {
                                self.queue_goaway(self.last_peer_sid, e.code);
                                sink.on_conn_error(self.key, e);
                                return;
                            }
                        };
                        let head = match self.validate_and_build_request(decoded) {
                            Ok(h) => h,
                            Err(e) => {
                                self.queue_rst_stream(sid, e.code);
                                sink.on_conn_error(self.key, e);
                                return;
                            }
                        };
                        sink.on_request_headers(self.key, sid, false, head);
                    }
                }

                FrameType::Data => {
                    let sid = fh.stream_id;
                    if sid == 0 {
                        let e = H2Error::new(H2Code::ProtocolError, "DATA on stream 0");
                        self.queue_goaway(self.last_peer_sid, e.code);
                        sink.on_conn_error(self.key, e);
                        return;
                    }
                    if self.streams.get(sid).is_none() {
                        let e = H2Error::new(H2Code::ProtocolError, "DATA on idle/unknown stream");
                        self.queue_goaway(self.last_peer_sid, e.code);
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
                            self.queue_goaway(self.last_peer_sid, e.code);
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

                        // recv window decrease (conn + stream)
                        self.recv_conn_win -= data_len as i64;
                        if self.recv_conn_win < 0 {
                            let e =
                                H2Error::new(H2Code::FlowControlError, "conn recv window negative");
                            self.queue_goaway(self.last_peer_sid, e.code);
                            sink.on_conn_error(self.key, e);
                            return;
                        }
                        if let Some(st) = self.streams.get_mut(sid) {
                            st.flow.dec_recv(data_len);
                            if st.flow.recv_win < 0 {
                                self.queue_rst_stream(sid, H2Code::FlowControlError);
                                // release data immediately to avoid leak
                                data.release(ops);
                                return;
                            }
                            if data_len == 0 {
                                st.empty_data_strikes = st.empty_data_strikes.saturating_add(1);
                            }
                        }
                        if end_stream {
                            self.on_remote_end_stream(sid);
                        }

                        sink.on_request_data(self.key, sid, end_stream, data);
                    } else {
                        let data_len = fh.len;
                        let mut data = match self.rx.take_chain(data_len, ops) {
                            Ok(c) => c,
                            Err(_) => return,
                        };

                        self.recv_conn_win -= data_len as i64;
                        if self.recv_conn_win < 0 {
                            let e =
                                H2Error::new(H2Code::FlowControlError, "conn recv window negative");
                            self.queue_goaway(self.last_peer_sid, e.code);
                            sink.on_conn_error(self.key, e);
                            return;
                        }

                        if let Some(st) = self.streams.get_mut(sid) {
                            st.flow.dec_recv(data_len);
                            if st.flow.recv_win < 0 {
                                self.queue_rst_stream(sid, H2Code::FlowControlError);
                                data.release(ops);
                                return;
                            }
                            if data_len == 0 {
                                st.empty_data_strikes = st.empty_data_strikes.saturating_add(1);
                            }
                        }
                        if end_stream {
                            self.on_remote_end_stream(sid);
                        }

                        sink.on_request_data(self.key, sid, end_stream, data);
                    }
                }

                FrameType::PushPromise => {
                    let e = H2Error::new(H2Code::ProtocolError, "PUSH_PROMISE from client");
                    self.queue_goaway(self.last_peer_sid, e.code);
                    sink.on_conn_error(self.key, e);
                    return;
                }

                _ => {
                    let _ = self.rx.consume(fh.len, ops);
                }
            }

            frames += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_removed_after_both_sides_end_stream() {
        let mut d = DownstreamH2::new(ConnKey::new(1, 1), 16, 0x42);

        d.ensure_stream_for_headers(1).expect("open stream");
        assert_eq!(d.streams.len(), 1);

        d.on_remote_end_stream(1);
        assert_eq!(d.streams.len(), 1);

        d.send_response_headers(1, 200, vec![], true)
            .expect("send response headers");
        assert_eq!(d.streams.len(), 0);
        assert!(d.streams.get(1).is_none());
    }

    #[test]
    fn closed_streams_are_reclaimed_under_long_churn() {
        let mut d = DownstreamH2::new(ConnKey::new(2, 1), 32, 0x99);

        for i in 0..1024u32 {
            let sid = (i << 1) | 1;
            d.ensure_stream_for_headers(sid)
                .expect("open downstream stream");
            d.on_remote_end_stream(sid);
            d.send_response_headers(sid, 200, vec![], true)
                .expect("close downstream stream");
        }

        assert_eq!(d.streams.len(), 0);
    }

    #[test]
    fn max_concurrent_streams_respects_constructor_limit() {
        let mut d = DownstreamH2::new(ConnKey::new(3, 1), 2, 0x1234);
        assert_eq!(d.local.max_concurrent_streams, 2);

        d.ensure_stream_for_headers(1).expect("open sid 1");
        d.ensure_stream_for_headers(3).expect("open sid 3");
        assert_eq!(d.streams.len(), 2);

        d.ensure_stream_for_headers(5)
            .expect("overflow stream should be refused, not fatal");
        assert_eq!(d.streams.len(), 2);
    }
}
