use uuid::Uuid;

/// W3C Trace Context (traceparent).
#[derive(Debug, Clone, Copy)]
pub struct TraceContext {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub flags: u8,
}

impl TraceContext {
    /// Create a new root trace context.
    pub fn new() -> Self {
        let u = Uuid::new_v4();
        let mut trace_id = [0u8; 16];
        trace_id.copy_from_slice(u.as_bytes());

        let mut span_id = [0u8; 8];
        span_id.copy_from_slice(&trace_id[..8]);

        Self {
            trace_id,
            span_id,
            flags: 1,
        }
    }

    pub fn traceparent(&self) -> String {
        // version 00
        format!(
            "00-{}-{}-{:02x}",
            hex16(self.trace_id),
            hex8(self.span_id),
            self.flags
        )
    }
}

fn hex16(b: [u8; 16]) -> String {
    let mut s = String::with_capacity(32);
    for x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}
fn hex8(b: [u8; 8]) -> String {
    let mut s = String::with_capacity(16);
    for x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}
