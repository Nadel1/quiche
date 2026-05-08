// Copyright (c) 2015 The Chromium Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Copyright (C) 2023, Cloudflare, Inc.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are
// met:
//
//     * Redistributions of source code must retain the above copyright notice,
//       this list of conditions and the following disclaimer.
//
//     * Redistributions in binary form must reproduce the above copyright
//       notice, this list of conditions and the following disclaimer in the
//       documentation and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
// IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO,
// THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
// PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR
// CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
// EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
// PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
// PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
// LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
// NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
// SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use std::time::Duration;
use std::time::Instant;

use crate::recovery::gcongestion::bbr2::Params;
use crate::recovery::gcongestion::Acked;
use crate::recovery::gcongestion::Lost;
use crate::recovery::Bandwidth;
use crate::recovery::RecoveryStats;

use super::mode::Cycle;
use super::mode::Mode;
use super::mode::ModeImpl;
use super::network_model::BBRv2NetworkModel;
use super::BBRv2CongestionEvent;
use super::Limits;

#[derive(Debug)]
pub(super) struct Drain {
    pub(super) model: BBRv2NetworkModel,
    pub(super) cycle: Cycle,
}

impl ModeImpl for Drain {
    #[cfg(feature = "qlog")]
    fn state_str(&self) -> &'static str {
        "bbr_drain"
    }

    fn is_probing_for_bandwidth(&self) -> bool {
        false
    }

    fn on_congestion_event(
        mut self, _prior_in_flight: usize, event_time: Instant,
        _acked_packets: &[Acked], _lost_packets: &[Lost],
        congestion_event: &mut BBRv2CongestionEvent,
        _target_bytes_inflight: usize, params: &Params,
        _recovery_stats: &mut RecoveryStats, _cwnd: usize,
    ) -> Mode {
        let logging_values = vec![
            self.model.rounds_with_queueing() as u128,
            self.model.min_bytes_in_flight_in_round() as u128,
            self.model.cwnd_gain() as u128,
            self.model.pacing_gain() as u128,
            self.model.inflight_hi() as u128,
            congestion_event.event_time.elapsed().as_millis() as u128,
            congestion_event.prior_cwnd as u128,
            congestion_event.prior_bytes_in_flight as u128,
            congestion_event.bytes_in_flight as u128,
            congestion_event.bytes_acked as u128,
            congestion_event.bytes_lost as u128,
            congestion_event.end_of_round_trip as u128,
            congestion_event.is_probing_for_bandwidth as u128,
            congestion_event
                .sample_max_bandwidth
                .unwrap_or(Bandwidth { bits_per_second: 0 })
                .bits_per_second as u128,
            congestion_event
                .sample_min_rtt
                .unwrap_or(Duration::from_millis(0))
                .as_millis(),
            congestion_event.last_packet_send_state.is_valid as u128,
            congestion_event.last_packet_send_state.is_app_limited as u128,
            congestion_event.last_packet_send_state.total_bytes_sent as u128,
            congestion_event.last_packet_send_state.total_bytes_acked as u128,
            congestion_event.last_packet_send_state.total_bytes_lost as u128,
            congestion_event.last_packet_send_state.bytes_in_flight as u128,
            false as u128,
            self.model.loss_events_in_round() as u128,
            self.model.get_total_acked_bytes() as u128,
            self.model
                .bandwidth_lo
                .unwrap_or(Bandwidth::infinite())
                .bits_per_second as u128,
            self.model.round_trip_count() as u128,
        ];

        self.model.write_to_log("DRAIN".to_owned(), logging_values);
        self.model.set_pacing_gain(params.drain_pacing_gain);
        // Only STARTUP can transition to DRAIN, both of them use the same cwnd
        // gain.
        self.model.set_cwnd_gain(params.drain_cwnd_gain);

        let drain_target = self.drain_target();
        if congestion_event.bytes_in_flight <= drain_target {
            return self.into_probe_bw(
                event_time,
                Some(congestion_event),
                params,
            );
        }

        Mode::Drain(self)
    }

    fn get_cwnd_limits(&self, _params: &Params) -> Limits<usize> {
        Limits {
            lo: 0,
            hi: self.model.inflight_lo(),
        }
    }

    fn on_exit_quiescence(
        self, _now: Instant, _quiescence_start_time: Instant, _params: &Params,
    ) -> Mode {
        Mode::Drain(self)
    }

    fn enter(
        &mut self, _: Instant, _: Option<&BBRv2CongestionEvent>, _params: &Params,
    ) {
    }

    fn leave(&mut self, _: Instant, _: Option<&BBRv2CongestionEvent>) {}
}

impl Drain {
    fn into_probe_bw(
        mut self, now: Instant, congestion_event: Option<&BBRv2CongestionEvent>,
        params: &Params,
    ) -> Mode {
        self.leave(now, congestion_event);
        let mut next_mode = Mode::probe_bw(self.model, self.cycle);
        next_mode.enter(now, congestion_event, params);
        next_mode
    }

    fn drain_target(&self) -> usize {
        self.model.bdp0()
    }
}
