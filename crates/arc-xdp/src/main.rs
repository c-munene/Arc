#![no_std]
#![no_main]

use core::mem;

use aya_ebpf::{
    bindings::xdp_action,
    helpers::gen::bpf_ktime_get_ns,
    macros::{map, xdp},
    maps::{Array, HashMap, LruHashMap, RingBuf},
    programs::XdpContext,
};

use arc_xdp_common::{BlacklistEntry, IpKey, XdpConfig};

const ETH_P_IP: u16 = 0x0800;
const ETH_P_IPV6: u16 = 0x86DD;
const ETH_P_8021Q: u16 = 0x8100;
const ETH_P_8021AD: u16 = 0x88A8;

#[repr(C)]
#[derive(Copy, Clone)]
struct EthHdr {
    dst: [u8; 6],
    src: [u8; 6],
    proto: u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct VlanHdr {
    tci: u16,
    proto: u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct Ipv4Hdr {
    version_ihl: u8,
    tos: u8,
    tot_len: u16,
    id: u16,
    frag_off: u16,
    ttl: u8,
    proto: u8,
    checksum: u16,
    saddr: u32,
    daddr: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct Ipv6Hdr {
    vtc_flow: [u8; 4],
    payload_len: u16,
    next_header: u8,
    hop_limit: u8,
    saddr: [u8; 16],
    daddr: [u8; 16],
}

#[map(name = "arc_whitelist")]
static mut WHITELIST: HashMap<IpKey, u8> = HashMap::with_max_entries(65_536, 0);

#[map(name = "arc_blacklist")]
static mut BLACKLIST: LruHashMap<IpKey, BlacklistEntry> =
    LruHashMap::with_max_entries(1_000_000, 0);

#[map(name = "arc_config")]
static mut CONFIG: Array<XdpConfig> = Array::with_max_entries(1, 0);

#[map(name = "arc_events")]
static mut EVENTS: RingBuf = RingBuf::with_byte_size(256 * 1024, 0);

#[xdp]
pub fn arc_xdp(ctx: XdpContext) -> u32 {
    match try_arc_xdp(&ctx) {
        Ok(v) => v,
        Err(_) => xdp_action::XDP_PASS,
    }
}

#[inline(always)]
fn try_arc_xdp(ctx: &XdpContext) -> Result<u32, ()> {
    let mut l3_offset = mem::size_of::<EthHdr>();
    let eth = ptr_at::<EthHdr>(ctx, 0).ok_or(())?;
    let mut proto = unsafe { u16::from_be((*eth).proto) };

    if proto == ETH_P_8021Q || proto == ETH_P_8021AD {
        let vlan = ptr_at::<VlanHdr>(ctx, l3_offset).ok_or(())?;
        proto = unsafe { u16::from_be((*vlan).proto) };
        l3_offset += mem::size_of::<VlanHdr>();
    }

    if proto == ETH_P_IP {
        let ip4 = ptr_at::<Ipv4Hdr>(ctx, l3_offset).ok_or(())?;
        let ihl = unsafe { ((*ip4).version_ihl & 0x0f) as usize * 4 };
        if ihl < mem::size_of::<Ipv4Hdr>() {
            return Err(());
        }
        if !range_in_packet(ctx, l3_offset, ihl) {
            return Err(());
        }

        // `saddr` is in network byte order in packet memory.
        // Reading it as `u32` on little-endian host requires `to_ne_bytes`
        // to preserve the original 4-byte wire layout.
        let src = unsafe { (*ip4).saddr.to_ne_bytes() };
        let key = IpKey::from_ipv4_exact(src);
        return decide(key);
    }

    if proto == ETH_P_IPV6 {
        let ip6 = ptr_at::<Ipv6Hdr>(ctx, l3_offset).ok_or(())?;
        let key = IpKey::from_ipv6_exact(unsafe { (*ip6).saddr });
        return decide(key);
    }

    Ok(xdp_action::XDP_PASS)
}

#[inline(always)]
fn decide(key: IpKey) -> Result<u32, ()> {
    if unsafe { WHITELIST.get(&key) }.is_some() {
        return Ok(xdp_action::XDP_PASS);
    }

    if let Some(entry) = unsafe { BLACKLIST.get(&key).map(|v| *v) } {
        if entry.ttl_ns == 0 {
            return Ok(xdp_action::XDP_DROP);
        }

        let now = ktime_get_ns();
        let elapsed = now.wrapping_sub(entry.blocked_at_ns);
        if elapsed < entry.ttl_ns {
            return Ok(xdp_action::XDP_DROP);
        }

        let _ = unsafe { BLACKLIST.remove(&key) };
    }

    Ok(xdp_action::XDP_PASS)
}

#[inline(always)]
fn ptr_at<T>(ctx: &XdpContext, offset: usize) -> Option<*const T> {
    let start = ctx.data();
    let end = ctx.data_end();
    let p = start.saturating_add(offset);
    let size = mem::size_of::<T>();
    if p.saturating_add(size) > end {
        return None;
    }
    Some(p as *const T)
}

#[inline(always)]
fn range_in_packet(ctx: &XdpContext, offset: usize, len: usize) -> bool {
    let start = ctx.data();
    let end = ctx.data_end();
    let p = start.saturating_add(offset);
    p.saturating_add(len) <= end
}

#[inline(always)]
fn ktime_get_ns() -> u64 {
    unsafe { bpf_ktime_get_ns() as u64 }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
static LICENSE: [u8; 13] = *b"Dual MIT/GPL\0";
