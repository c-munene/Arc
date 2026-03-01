//! Arc Gossip State Bus (SWIM + Gossip).
//!
//! Design goals (per spec):
//! - Fully async on the dedicated control-plane Tokio runtime (never blocks io_uring workers).
//! - UDP for gossip messages (idempotent, loss-tolerant), TCP for full-state sync at join time.
//! - MessagePack serialization (self-implemented minimal codec; no external MsgPack deps).
//! - Only two CRDTs are implemented: G-Counter and LWW-Register.
//!
//! This module intentionally does **not** modify arc-config schema. Instead it parses
//! `cluster.gossip` and `cluster.fallback` from `ConfigManager.current().raw_json` (serde_json Value).
//! Unknown config fields are ignored by Arc's existing config compiler, so this is schema-compatible.

mod bus;
mod config;
mod message;
mod msgpack;

pub use bus::{GossipBus, GossipMemberView, GossipStatsView};
