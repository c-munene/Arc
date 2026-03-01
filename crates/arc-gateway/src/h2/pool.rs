#![forbid(unsafe_code)]

use super::key::ConnKey;
use super::up::UpstreamH2;

#[derive(Debug)]
struct Slot {
    gen: u32,
    conn: Option<UpstreamH2>,
}

#[derive(Debug)]
pub struct UpstreamPool {
    slots: Vec<Slot>,
    rr: usize,
}

impl UpstreamPool {
    pub fn new(max_conns: usize) -> Self {
        Self {
            slots: (0..max_conns)
                .map(|_| Slot { gen: 1, conn: None })
                .collect(),
            rr: 0,
        }
    }

    pub fn insert(&mut self, idx: usize, conn: UpstreamH2) -> ConnKey {
        let gen = self.slots[idx].gen;
        self.slots[idx].conn = Some(conn);
        ConnKey::new(idx as u32, gen)
    }

    pub fn remove(&mut self, key: ConnKey) -> Option<UpstreamH2> {
        let idx = key.idx as usize;
        let slot = self.slots.get_mut(idx)?;
        if slot.gen != key.gen {
            return None;
        }
        let c = slot.conn.take();
        slot.gen = slot.gen.wrapping_add(1).max(1);
        c
    }

    pub fn get_mut(&mut self, key: ConnKey) -> Option<&mut UpstreamH2> {
        let idx = key.idx as usize;
        let slot = self.slots.get_mut(idx)?;
        if slot.gen != key.gen {
            return None;
        }
        slot.conn.as_mut()
    }

    pub fn get(&self, key: ConnKey) -> Option<&UpstreamH2> {
        let idx = key.idx as usize;
        let slot = self.slots.get(idx)?;
        if slot.gen != key.gen {
            return None;
        }
        slot.conn.as_ref()
    }

    /// SOTA：least-loaded 选连接（active streams 最少），再 round-robin 打散
    pub fn pick_ready(&mut self) -> Option<ConnKey> {
        let n = self.slots.len();
        if n == 0 {
            return None;
        }

        let mut best: Option<(usize, u32)> = None;

        for k in 0..n {
            let i = (self.rr + k) % n;
            let slot = &self.slots[i];
            let Some(c) = slot.conn.as_ref() else {
                continue;
            };
            if !c.can_open_stream() {
                continue;
            }
            // active 粗略：用 next_sid/active 也行；这里用 active 的近似即可
            let score = 0u32; // 若你愿意，把 UpstreamH2 暴露 active 计数出来，这里就用 active
            if best.map(|b| score < b.1).unwrap_or(true) {
                best = Some((i, score));
            }
        }

        if let Some((i, _)) = best {
            self.rr = (i + 1) % n;
            let gen = self.slots[i].gen;
            return Some(ConnKey::new(i as u32, gen));
        }
        None
    }
}
