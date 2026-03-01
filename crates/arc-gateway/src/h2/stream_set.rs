#![forbid(unsafe_code)]

use super::flow::Flow;
#[cfg(feature = "h2-native-upstream")]
use super::key::ConnKey;
#[cfg(feature = "h2-native-upstream")]
use super::timewheel::Deadline;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    Idle,
    Open,
    HalfClosedLocal,
    HalfClosedRemote,
    Closed,
}

#[cfg(feature = "h2-native-upstream")]
#[derive(Clone, Copy, Debug)]
pub struct UpLink {
    pub up_conn: ConnKey,
    pub up_sid: u32,
}

#[derive(Debug)]
pub struct Stream {
    pub sid: u32,
    pub state: StreamState,

    /// send_win: 对端（peer）允许我们发送的窗口
    /// recv_win: 我们允许对端发送的窗口（只有 credit 后才 WINDOW_UPDATE）
    pub flow: Flow,

    #[cfg(feature = "h2-native-upstream")]
    pub deadline: Deadline,
    pub empty_data_strikes: u8,

    #[cfg(feature = "h2-native-upstream")]
    pub uplink: Option<UpLink>,
}

impl Stream {
    pub fn new(sid: u32, init_send: u32, init_recv: u32) -> Self {
        Self {
            sid,
            state: StreamState::Idle,
            flow: Flow::new(init_send, init_recv),
            #[cfg(feature = "h2-native-upstream")]
            deadline: Deadline::none(),
            empty_data_strikes: 0,
            #[cfg(feature = "h2-native-upstream")]
            uplink: None,
        }
    }
}

/// 极快 u32->index map：开地址 + tombstone + 自动 rehash
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
        let mut x = (k as u64) ^ self.seed;
        x = x.wrapping_mul(0x9e3779b97f4a7c15);
        (x as usize) & self.mask
    }

    fn should_rehash(&self) -> bool {
        // load factor（含 tombstone）过高就 rehash
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
pub struct StreamSet {
    v: Vec<Stream>,
    m: U32Map,
    cap: usize,
}

impl StreamSet {
    pub fn new(max_concurrent: usize, seed: u64) -> Self {
        let cap = max_concurrent.max(1);
        let map_cap = (cap * 2).next_power_of_two();
        Self {
            v: Vec::with_capacity(cap),
            m: U32Map::with_capacity_pow2(map_cap, seed),
            cap,
        }
    }

    pub fn len(&self) -> usize {
        self.v.len()
    }
    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn get_mut(&mut self, sid: u32) -> Option<&mut Stream> {
        let idx = self.m.get(sid)? as usize;
        self.v.get_mut(idx)
    }

    pub fn get(&self, sid: u32) -> Option<&Stream> {
        let idx = self.m.get(sid)? as usize;
        self.v.get(idx)
    }

    pub fn insert(&mut self, s: Stream) -> bool {
        if self.v.len() >= self.cap {
            return false;
        }
        let sid = s.sid;
        if self.m.get(sid).is_some() {
            return false;
        }
        let idx = self.v.len() as u32;
        self.v.push(s);
        self.m.insert(sid, idx)
    }

    pub fn remove(&mut self, sid: u32) -> Option<Stream> {
        let idx = self.m.remove(sid)? as usize;
        let last = self.v.len() - 1;
        let removed = self.v.swap_remove(idx);
        if idx != last {
            let moved_sid = self.v[idx].sid;
            self.m.update_value(moved_sid, idx as u32);
        }
        Some(removed)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Stream> {
        self.v.iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u32map_tombstone_rehash_stays_bounded() {
        let mut m = U32Map::with_capacity_pow2(16, 0x1234);
        let initial_cap = m.keys.len();

        for i in 0..20_000u32 {
            let sid = (i << 1) | 1;
            assert!(m.insert(sid, i));
            assert_eq!(m.get(sid), Some(i));
            assert_eq!(m.remove(sid), Some(i));
        }

        // Tombstone cleanup should rehash in-place; map capacity must not explode.
        assert!(m.keys.len() <= initial_cap * 2);
    }

    #[test]
    fn streamset_churn_keeps_index_map_bounded() {
        let mut set = StreamSet::new(32, 0x9abc);
        let initial_cap = set.m.keys.len();

        for i in 0..10_000u32 {
            let sid = (i << 1) | 1;
            let mut st = Stream::new(sid, 65_535, 65_535);
            st.state = StreamState::Open;
            assert!(set.insert(st));
            assert!(set.remove(sid).is_some());
        }

        assert_eq!(set.len(), 0);
        assert!(set.m.keys.len() <= initial_cap * 2);
    }
}
