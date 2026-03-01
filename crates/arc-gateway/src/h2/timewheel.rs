#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline(Option<u64>);

impl Deadline {
    #[cfg(feature = "h2-native-upstream")]
    pub const fn none() -> Self {
        Self(None)
    }

    #[cfg(feature = "h2-native-upstream")]
    pub const fn at(ts_ns: u64) -> Self {
        Self(Some(ts_ns))
    }

    #[cfg(feature = "h2-native-upstream")]
    pub const fn as_nanos(self) -> Option<u64> {
        self.0
    }
}
