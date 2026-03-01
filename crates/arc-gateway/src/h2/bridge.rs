#![forbid(unsafe_code)]

use arc_proto_h2::{
    error::{H2Code, H2Error},
    hpack::Header,
};

use bytes::Bytes;

use super::{
    buf::{BufChain, BufOps},
    down::{DownstreamH2, DownstreamSink, RequestHead},
    key::ConnKey,
    pool::UpstreamPool,
    stream_set::UpLink,
    tx::Credit,
    up::{DownLink, UpstreamSink},
};

use std::collections::VecDeque;

#[derive(Debug)]
enum Op {
    DownHeaders {
        down: ConnKey,
        sid: u32,
        end_stream: bool,
        head: RequestHead,
    },
    DownData {
        down: ConnKey,
        sid: u32,
        end_stream: bool,
        data: BufChain,
    },
    DownRst {
        down: ConnKey,
        sid: u32,
        code: H2Code,
    },

    UpHeaders {
        up: ConnKey,
        up_sid: u32,
        end_stream: bool,
        status: u16,
        headers: Vec<Header>,
    },
    UpData {
        up: ConnKey,
        up_sid: u32,
        end_stream: bool,
        data: BufChain,
    },
    UpRst {
        up: ConnKey,
        up_sid: u32,
        code: H2Code,
    },

    UpGoaway {
        up: ConnKey,
        last_sid: u32,
        code: H2Code,
    },
}

#[derive(Debug)]
pub struct Bridge {
    q: VecDeque<Op>,
}

impl Bridge {
    pub fn new() -> Self {
        Self { q: VecDeque::new() }
    }

    /// 执行桥接操作：必须在 worker 单线程里调用
    pub fn drain(
        &mut self,
        ops: &mut dyn BufOps,
        downs: &mut dyn DownConnStore,
        ups: &mut UpstreamPool,
    ) {
        while let Some(op) = self.q.pop_front() {
            match op {
                Op::DownHeaders {
                    down,
                    sid,
                    end_stream,
                    head,
                } => {
                    let Some(d) = downs.get_mut(down) else {
                        continue;
                    };

                    // 选 upstream conn（pool 自己保证 can_open_stream）
                    let Some(up_key) = ups.pick_ready() else {
                        let _ = d.send_response_headers(sid, 503, vec![], true);
                        continue;
                    };
                    let Some(u) = ups.get_mut(up_key) else {
                        let _ = d.send_response_headers(sid, 503, vec![], true);
                        continue;
                    };

                    // open upstream stream
                    let up_sid = match u.open_stream(DownLink {
                        down,
                        down_sid: sid,
                    }) {
                        Ok(x) => x,
                        Err(_e) => {
                            let _ = d.send_response_headers(sid, 503, vec![], true);
                            continue;
                        }
                    };

                    // bind mapping (down -> up)
                    d.bind_uplink(
                        sid,
                        UpLink {
                            up_conn: up_key,
                            up_sid,
                        },
                    );

                    // build upstream request headers (pseudo first)
                    let mut hs: Vec<Header> = Vec::with_capacity(head.headers.len() + 4);

                    hs.push(Header {
                        name: Bytes::from_static(b":method"),
                        value: head.method,
                    });

                    if let Some(scheme) = head.scheme {
                        hs.push(Header {
                            name: Bytes::from_static(b":scheme"),
                            value: scheme,
                        });
                    }
                    // :authority 优先使用伪头；缺失时兜底用 host header
                    if let Some(auth) = head.authority {
                        hs.push(Header {
                            name: Bytes::from_static(b":authority"),
                            value: auth,
                        });
                    } else {
                        let mut host: Option<Bytes> = None;
                        for h in head.headers.iter() {
                            if h.name.as_ref() == b"host" {
                                host = Some(h.value.clone());
                                break;
                            }
                        }
                        if let Some(hv) = host {
                            hs.push(Header {
                                name: Bytes::from_static(b":authority"),
                                value: hv,
                            });
                        }
                    }

                    if let Some(path) = head.path {
                        hs.push(Header {
                            name: Bytes::from_static(b":path"),
                            value: path,
                        });
                    }

                    hs.extend(head.headers);

                    if u.send_request_headers(up_sid, end_stream, hs).is_err() {
                        let _ = d.send_response_headers(sid, 503, vec![], true);
                        let _ = u.send_rst_stream(up_sid, H2Code::Cancel);
                        u.close_stream(up_sid);
                        continue;
                    }
                }

                Op::DownData {
                    down,
                    sid,
                    end_stream,
                    mut data,
                } => {
                    let Some(d) = downs.get_mut(down) else {
                        data.release(ops);
                        continue;
                    };

                    let Some(link) = d.uplink(sid) else {
                        // DATA before uplink ready => fast-fail
                        let _ = d.send_response_headers(sid, 502, vec![], true);
                        data.release(ops);
                        continue;
                    };

                    let Some(u) = ups.get_mut(link.up_conn) else {
                        let _ = d.send_response_headers(sid, 503, vec![], true);
                        data.release(ops);
                        continue;
                    };

                    let bytes = data.total_len();
                    let credit = Some(Credit::ToDownstream {
                        conn: down,
                        sid,
                        bytes,
                    });

                    if u.send_request_data(link.up_sid, end_stream, data, credit, ops)
                        .is_err()
                    {
                        let _ = d.send_response_headers(sid, 503, vec![], true);
                        let _ = u.send_rst_stream(link.up_sid, H2Code::Cancel);
                        u.close_stream(link.up_sid);
                        // data 已经被传入 send_request_data，内部要么入队要么 release，不在这里重复 release
                        continue;
                    }
                }

                Op::DownRst { down, sid, code: _ } => {
                    let Some(d) = downs.get_mut(down) else {
                        continue;
                    };

                    if let Some(link) = d.uplink(sid) {
                        if let Some(u) = ups.get_mut(link.up_conn) {
                            let _ = u.send_rst_stream(link.up_sid, H2Code::Cancel);
                            u.close_stream(link.up_sid);
                        }
                    }
                }

                Op::UpHeaders {
                    up,
                    up_sid,
                    end_stream,
                    status,
                    headers,
                } => {
                    let Some(u) = ups.get_mut(up) else {
                        continue;
                    };
                    let Some(dl) = u.downlink(up_sid) else {
                        continue;
                    };
                    let Some(d) = downs.get_mut(dl.down) else {
                        continue;
                    };

                    let _ = d.send_response_headers(dl.down_sid, status, headers, end_stream);

                    if end_stream {
                        u.close_stream(up_sid);
                    }
                }

                Op::UpData {
                    up,
                    up_sid,
                    end_stream,
                    mut data,
                } => {
                    let Some(u) = ups.get_mut(up) else {
                        data.release(ops);
                        continue;
                    };
                    let Some(dl) = u.downlink(up_sid) else {
                        data.release(ops);
                        continue;
                    };
                    let Some(d) = downs.get_mut(dl.down) else {
                        data.release(ops);
                        continue;
                    };

                    let bytes = data.total_len();
                    let credit = Some(Credit::ToUpstream {
                        conn: up,
                        sid: up_sid,
                        bytes,
                    });

                    if d.send_response_data(dl.down_sid, end_stream, data, credit, ops)
                        .is_err()
                    {
                        // 下游写不出去：取消 upstream
                        let _ = u.send_rst_stream(up_sid, H2Code::Cancel);
                        u.close_stream(up_sid);
                        continue;
                    }

                    if end_stream {
                        u.close_stream(up_sid);
                    }
                }

                Op::UpRst {
                    up,
                    up_sid,
                    code: _,
                } => {
                    let Some(u) = ups.get_mut(up) else {
                        continue;
                    };

                    if let Some(dl) = u.downlink(up_sid) {
                        if let Some(d) = downs.get_mut(dl.down) {
                            // 无法稳定判断“下游是否已发过 response headers”，最安全是直接 502 + end_stream
                            // 如果你愿意进一步 SOTA：给 DownstreamH2 增加 per-stream responded 标志位，再决定是 RST 还是 502
                            let _ = d.send_response_headers(dl.down_sid, 502, vec![], true);
                        }
                    }

                    u.close_stream(up_sid);
                }

                Op::UpGoaway { up, last_sid, code } => {
                    // 标记上游连接 draining，pool 不再挑它开新流
                    if let Some(u) = ups.get_mut(up) {
                        u.mark_goaway(last_sid, code);
                    }
                }
            }
        }
    }
}

pub trait DownConnStore {
    fn get_mut(&mut self, key: ConnKey) -> Option<&mut DownstreamH2>;
}

// -------- Bridge implements event sinks: only enqueue ops (no borrow conflicts) --------

impl DownstreamSink for Bridge {
    fn on_request_headers(&mut self, down: ConnKey, sid: u32, end_stream: bool, head: RequestHead) {
        self.q.push_back(Op::DownHeaders {
            down,
            sid,
            end_stream,
            head,
        });
    }

    fn on_request_data(&mut self, down: ConnKey, sid: u32, end_stream: bool, data: BufChain) {
        self.q.push_back(Op::DownData {
            down,
            sid,
            end_stream,
            data,
        });
    }

    fn on_rst_stream(&mut self, down: ConnKey, sid: u32, code: H2Code) {
        self.q.push_back(Op::DownRst { down, sid, code });
    }

    fn on_goaway(&mut self, _down: ConnKey, _last_sid: u32, _code: H2Code) {}

    fn on_conn_error(&mut self, _down: ConnKey, _err: H2Error) {}
}

impl UpstreamSink for Bridge {
    fn on_response_headers(
        &mut self,
        up: ConnKey,
        up_sid: u32,
        end_stream: bool,
        status: u16,
        headers: Vec<Header>,
    ) {
        self.q.push_back(Op::UpHeaders {
            up,
            up_sid,
            end_stream,
            status,
            headers,
        });
    }

    fn on_response_data(&mut self, up: ConnKey, up_sid: u32, end_stream: bool, data: BufChain) {
        self.q.push_back(Op::UpData {
            up,
            up_sid,
            end_stream,
            data,
        });
    }

    fn on_rst_stream(&mut self, up: ConnKey, up_sid: u32, code: H2Code) {
        self.q.push_back(Op::UpRst { up, up_sid, code });
    }

    fn on_goaway(&mut self, up: ConnKey, last_sid: u32, code: H2Code) {
        self.q.push_back(Op::UpGoaway { up, last_sid, code });
    }

    fn on_conn_error(&mut self, _up: ConnKey, _err: H2Error) {}
}
