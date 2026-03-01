#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flow {
    pub send_win: i64,
    pub recv_win: i64,
}

impl Flow {
    pub fn new(init_send: u32, init_recv: u32) -> Self {
        Self {
            send_win: init_send as i64,
            recv_win: init_recv as i64,
        }
    }

    pub fn inc_send(&mut self, n: u32) {
        self.send_win += n as i64;
    }

    pub fn dec_send(&mut self, n: u32) {
        self.send_win -= n as i64;
    }

    pub fn inc_recv(&mut self, n: u32) {
        self.recv_win += n as i64;
    }

    pub fn dec_recv(&mut self, n: u32) {
        self.recv_win -= n as i64;
    }
}
