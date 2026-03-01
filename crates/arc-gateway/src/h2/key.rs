#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConnKey {
    pub idx: u32,
    pub gen: u32,
}

impl ConnKey {
    pub const fn new(idx: u32, gen: u32) -> Self {
        Self { idx, gen }
    }
}
