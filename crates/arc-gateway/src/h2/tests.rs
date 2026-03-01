#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};

use bytes::Bytes;

use arc_proto_h2::frame::{flags, FrameHeader, FrameType, PREFACE};
use arc_proto_h2::hpack::{Header, HpackEncoder};

use super::buf::{BufOps, RxChunk, RxQueue};
use super::down::{DownstreamH2, DownstreamSink, RequestHead};
use super::driver::drain_tx_to_writer;
use super::key::ConnKey;
use super::tx::{Credit, TxItem};

#[derive(Default)]
struct MockOps {
    bufs: HashMap<u16, Vec<u8>>,
    refs: HashMap<u16, i32>,
}

impl MockOps {
    fn put(&mut self, id: u16, data: Vec<u8>) {
        self.bufs.insert(id, data);
        self.refs.insert(id, 1);
    }

    fn refs(&self, id: u16) -> i32 {
        *self.refs.get(&id).unwrap_or(&0)
    }
}

impl BufOps for MockOps {
    fn slice<'a>(&'a self, buf_id: u16, off: u32, len: u32) -> &'a [u8] {
        let b = self.bufs.get(&buf_id).expect("buffer exists");
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

fn frame(ty: FrameType, fl: u8, sid: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + payload.len());
    let mut hdr = [0u8; 9];
    FrameHeader {
        len: payload.len() as u32,
        ty,
        flags: fl,
        stream_id: sid,
    }
    .write_into(&mut hdr);
    out.extend_from_slice(&hdr);
    out.extend_from_slice(payload);
    out
}

enum Event {
    ReqHead {
        sid: u32,
        end_stream: bool,
        method: Bytes,
        path: Option<Bytes>,
    },
    ReqData {
        sid: u32,
        end_stream: bool,
        data: super::buf::BufChain,
    },
}

#[derive(Default)]
struct Sink {
    ev: Vec<Event>,
}

impl DownstreamSink for Sink {
    fn on_request_headers(
        &mut self,
        _down: ConnKey,
        sid: u32,
        end_stream: bool,
        head: RequestHead,
    ) {
        self.ev.push(Event::ReqHead {
            sid,
            end_stream,
            method: head.method,
            path: head.path,
        });
    }

    fn on_request_data(
        &mut self,
        _down: ConnKey,
        sid: u32,
        end_stream: bool,
        data: super::buf::BufChain,
    ) {
        self.ev.push(Event::ReqData {
            sid,
            end_stream,
            data,
        });
    }

    fn on_rst_stream(&mut self, _down: ConnKey, _sid: u32, _code: arc_proto_h2::error::H2Code) {}
    fn on_goaway(&mut self, _down: ConnKey, _last_sid: u32, _code: arc_proto_h2::error::H2Code) {}
    fn on_conn_error(&mut self, _down: ConnKey, _err: arc_proto_h2::error::H2Error) {}
}

#[test]
fn downstream_h2_parses_headers_and_zero_copy_data() {
    let mut enc = HpackEncoder::default();
    let headers = vec![
        Header {
            name: Bytes::from_static(b":method"),
            value: Bytes::from_static(b"GET"),
        },
        Header {
            name: Bytes::from_static(b":scheme"),
            value: Bytes::from_static(b"https"),
        },
        Header {
            name: Bytes::from_static(b":authority"),
            value: Bytes::from_static(b"localhost"),
        },
        Header {
            name: Bytes::from_static(b":path"),
            value: Bytes::from_static(b"/x"),
        },
    ];
    let hb = enc.encode_headers(&headers);
    let mut wire = Vec::new();
    wire.extend_from_slice(PREFACE);
    wire.extend_from_slice(&frame(FrameType::Settings, 0, 0, &[]));
    wire.extend_from_slice(&frame(
        FrameType::Headers,
        flags::END_HEADERS,
        1,
        hb.as_ref(),
    ));
    wire.extend_from_slice(&frame(FrameType::Data, flags::END_STREAM, 1, b"abc"));

    let mut ops = MockOps::default();
    ops.put(10, wire.clone());

    let key = ConnKey::new(7, 1);
    let mut d = DownstreamH2::new(key, 16, 0x1234);
    d.push_rx(RxChunk {
        buf_id: 10,
        off: 0,
        len: wire.len() as u32,
    });

    let mut sink = Sink::default();
    d.pump(0, &mut ops, &mut sink);

    assert!(sink.ev.len() >= 2);
    match &sink.ev[0] {
        Event::ReqHead {
            sid,
            end_stream,
            method,
            path,
        } => {
            assert_eq!(*sid, 1);
            assert!(!end_stream);
            assert_eq!(method.as_ref(), b"GET");
            assert_eq!(path.as_ref().map(|x| x.as_ref()), Some(&b"/x"[..]));
        }
        _ => panic!("expected request headers event"),
    }
    match &mut sink.ev[1] {
        Event::ReqData {
            sid,
            end_stream,
            data,
        } => {
            assert_eq!(*sid, 1);
            assert!(*end_stream);
            let mut payload = Vec::new();
            let mut first_id = None;
            for seg in data.iter() {
                if first_id.is_none() {
                    first_id = Some(seg.buf_id);
                }
                payload.extend_from_slice(ops.slice(seg.buf_id, seg.off, seg.len));
            }
            assert_eq!(first_id, Some(10));
            assert_eq!(payload.as_slice(), b"abc");
            data.release(&mut ops);
        }
        _ => panic!("expected request data event"),
    }
    assert_eq!(ops.refs(10), 0);
}

#[test]
fn driver_releases_payload_and_calls_credit() {
    let mut ops = MockOps::default();
    ops.put(5, b"XYZ".to_vec());

    let mut rx = RxQueue::new();
    rx.push_chunk(RxChunk {
        buf_id: 5,
        off: 0,
        len: 3,
    });
    let chain = rx.take_chain(3, &mut ops).expect("take_chain");

    let mut tx = VecDeque::new();
    tx.push_back(TxItem::FrameData {
        header: [0u8; 9],
        payload: chain,
        credit: Some(Credit::ToDownstream {
            conn: ConnKey::new(1, 1),
            sid: 3,
            bytes: 3,
        }),
    });

    let mut out = Vec::new();
    let mut credits = Vec::new();
    let n =
        drain_tx_to_writer(&mut tx, &mut ops, &mut out, |c| credits.push(c), 1024).expect("drain");
    assert_eq!(n, 12);
    assert!(tx.is_empty());
    assert_eq!(out.len(), 12);
    assert_eq!(ops.refs(5), 0);
    assert_eq!(credits.len(), 1);
}
