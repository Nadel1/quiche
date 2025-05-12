use std::time::Instant;

use super::rtt::RttStats;
use super::Acked;
use super::Sent;
use super::Congestion;
use super::CongestionControlOps;

pub(crate) static NOOP: CongestionControlOps = CongestionControlOps {
    on_init,
    on_packet_sent,
    on_packets_acked,
    congestion_event,
    checkpoint,
    rollback,
    has_custom_pacing,
    debug_fmt,
};

pub fn on_init(_r: &mut Congestion) {}

pub fn on_packet_sent(
    _r: &mut Congestion, _sent_bytes: usize, _bytes_in_flight: usize,
    _now: Instant,
) {
}

fn on_packets_acked(
    _r: &mut Congestion, _bytes_in_flight: usize, _packets: &mut Vec<Acked>,
    _now: Instant, _rtt_stats: &RttStats,
) {
}

fn congestion_event(
    _r: &mut Congestion, _bytes_in_flight: usize, _lost_bytes: usize,
    _largest_lost_pkt: &Sent, _now: Instant,
) {
}

fn checkpoint(_r: &mut Congestion) {}

fn rollback(_r: &mut Congestion) -> bool {
    true
}

fn has_custom_pacing() -> bool {
    false
}

fn debug_fmt(_r: &Congestion, _f: &mut std::fmt::Formatter) -> std::fmt::Result {
    Ok(())
}