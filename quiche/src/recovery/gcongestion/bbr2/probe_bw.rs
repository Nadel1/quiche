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

use std::ops::Add;
use std::time::Duration;
use std::time::Instant;

use crate::recovery::gcongestion::bbr2::Params;
use crate::recovery::gcongestion::Acked;
use crate::recovery::gcongestion::Lost;
use crate::recovery::Bandwidth;
use crate::recovery::RecoveryStats;

use super::mode::Cycle;
use super::mode::CyclePhase;
use super::mode::Mode;
use super::mode::ModeImpl;
use super::network_model::BBRv2NetworkModel;
use super::network_model::DEFAULT_MSS;
use super::BBRv2CongestionEvent;
use super::BwLoMode;
use super::Limits;

#[derive(Debug)]
pub(super) struct ProbeBW {
    pub(super) model: BBRv2NetworkModel,
    pub(super) cycle: Cycle,
}

#[derive(PartialEq, PartialOrd)]
enum AdaptUpperBoundsResult {
    AdaptedOk,
    AdaptedProbedTooHigh,
    NotAdaptedInflightHighNotSet,
    NotAdaptedInvalidSample,
}

impl ModeImpl for ProbeBW {
    #[cfg(feature = "qlog")]
    fn state_str(&self) -> &'static str {
        match self.cycle.phase {
            CyclePhase::NotStarted => unreachable!(),
            CyclePhase::Up => "bbr_probe_bw_up",
            CyclePhase::Down => "bbr_probe_bw_down",
            CyclePhase::Cruise => "bbr_probe_bw_cruise",
            CyclePhase::Refill => "bbr_probe_bw_refill",
        }
    }

    fn enter(
        &mut self, now: Instant,
        _congestion_event: Option<&BBRv2CongestionEvent>, params: &Params,
    ) {
        self.cycle.start_time = now;

        match self.cycle.phase {
            CyclePhase::NotStarted => {
                // First time entering PROBE_BW. Start a new probing cycle.
                self.enter_probe_down(false, false, now, params)
            },
            CyclePhase::Cruise => self.enter_probe_cruise(now),
            CyclePhase::Refill =>
                self.enter_probe_refill(self.cycle.probe_up_rounds, now),
            CyclePhase::Up | CyclePhase::Down => {},
        }
    }

    fn on_congestion_event(
        mut self, prior_in_flight: usize, event_time: Instant, _: &[Acked],
        _: &[Lost], congestion_event: &mut BBRv2CongestionEvent,
        target_bytes_inflight: usize, params: &Params,
        _recovery_stats: &mut RecoveryStats, cwnd: usize,
    ) -> Mode {
        if congestion_event.end_of_round_trip {
            if self.cycle.start_time != event_time {
                self.cycle.rounds_since_probe += 1;
            }

            if self.cycle.phase_start_time != event_time {
                self.cycle.rounds_in_phase += 1;
            }
        }

        let mut switch_to_probe_rtt = false;

        // let logging_values = vec![
        //    cwnd as u128,
        //    self.model.cwnd_gain() as u128,
        //    self.model.pacing_gain() as u128,
        //    congestion_event.end_of_round_trip as u128,
        //
        //];
        // let cycle_string:String;
        // match self.cycle.phase {
        //    CyclePhase::NotStarted => unreachable!(),
        //    CyclePhase::Up => cycle_string="UP".to_owned(),
        //    CyclePhase::Down => cycle_string="DOWN".to_owned(),
        //    CyclePhase::Cruise => cycle_string="CRUISE".to_owned(),
        //    CyclePhase::Refill => cycle_string="REFILL".to_owned(),
        //}
        // self.model.write_to_log(cycle_string, logging_values);

        match self.cycle.phase {
            CyclePhase::NotStarted => unreachable!(),
            CyclePhase::Up => self.update_probe_up(
                prior_in_flight,
                target_bytes_inflight,
                congestion_event,
                params,
            ),
            CyclePhase::Down => {
                self.update_probe_down(
                    target_bytes_inflight,
                    congestion_event,
                    params,
                );
                if self.cycle.phase != CyclePhase::Down &&
                    self.model.maybe_expire_min_rtt(congestion_event, params)
                {
                    switch_to_probe_rtt = true;
                    println!("Switch to probe rtt set to true");
                }
            },
            CyclePhase::Cruise => self.update_probe_cruise(
                target_bytes_inflight,
                congestion_event,
                params,
            ),
            CyclePhase::Refill => self.update_probe_refill(
                target_bytes_inflight,
                congestion_event,
                params,
            ),
        }

        // Do not need to set the gains if switching to PROBE_RTT, they will be
        // set when `ProbeRTT::enter` is called.
        if !switch_to_probe_rtt {
            self.model
                .set_pacing_gain(self.cycle.phase.pacing_gain(params));
            self.model.set_cwnd_gain(self.cycle.phase.cwnd_gain(params));
        }

        if switch_to_probe_rtt {
            self.into_probe_rtt(event_time, Some(congestion_event), params)
        } else {
            Mode::ProbeBW(self)
        }
    }

    fn get_cwnd_limits(&self, params: &Params) -> Limits<usize> {
        if self.cycle.phase == CyclePhase::Cruise {
            let limit = self
                .model
                .inflight_lo()
                .min(self.model.inflight_hi_with_headroom(params));
            return Limits::no_greater_than(limit);
        }

        if self.cycle.phase == CyclePhase::Up &&
            params.probe_up_ignore_inflight_hi
        {
            // Similar to STARTUP.
            return Limits::no_greater_than(self.model.inflight_lo());
        }

        Limits::no_greater_than(
            self.model.inflight_lo().min(self.model.inflight_hi()),
        )
    }

    fn is_probing_for_bandwidth(&self) -> bool {
        self.cycle.phase == CyclePhase::Refill ||
            self.cycle.phase == CyclePhase::Up
    }

    fn on_exit_quiescence(
        mut self, now: Instant, quiescence_start_time: Instant, _params: &Params,
    ) -> Mode {
        println!("On exit quiescence; afterwards postone_min_rtt with force update is called with parameter: {:?}",(now - quiescence_start_time).as_micros());
        self.model
            .postpone_min_rtt_timestamp(now - quiescence_start_time);
        Mode::ProbeBW(self)
    }

    fn leave(
        &mut self, _now: Instant,
        _congestion_event: Option<&BBRv2CongestionEvent>,
    ) {
    }
}

impl ProbeBW {
    fn enter_probe_down(
        &mut self, probed_too_high: bool, stopped_risky_probe: bool,
        now: Instant, params: &Params,
    ) {
        let cycle = &mut self.cycle;
        cycle.last_cycle_probed_too_high = probed_too_high;
        cycle.last_cycle_stopped_risky_probe = stopped_risky_probe;

        cycle.phase = CyclePhase::Down;
        cycle.start_time = now;
        cycle.phase_start_time = now;
        cycle.rounds_in_phase = 0;

        if params.bw_lo_mode != BwLoMode::Default {
            // Clear bandwidth lo if it was set in PROBE_UP, because losses in
            // PROBE_UP should not permanently change bandwidth_lo.
            // It's possible for bandwidth_lo to be set during REFILL, but if that
            // was a valid value, it'll quickly be rediscovered.
            self.model.clear_bandwidth_lo();
        }

        // Pick probe wait time.
        // TODO(vlad): actually pick time
        cycle.rounds_since_probe = 0;
        cycle.probe_wait_time = Some(
            params.probe_bw_probe_base_duration + Duration::from_micros(500),
        );

        cycle.probe_up_bytes = None;
        cycle.probe_up_app_limited_since_inflight_hi_limited = false;
        cycle.has_advanced_max_bw = false;
        self.model.restart_round_early();
    }

    fn enter_probe_cruise(&mut self, now: Instant) {
        if self.cycle.phase == CyclePhase::Down {
            self.exit_probe_down();
        }

        let cycle = &mut self.cycle;

        self.model.cap_inflight_lo(self.model.inflight_hi());
        cycle.phase = CyclePhase::Cruise;
        cycle.phase_start_time = now;
        cycle.rounds_in_phase = 0;
        cycle.is_sample_from_probing = false;
    }

    fn enter_probe_refill(&mut self, probe_up_rounds: usize, now: Instant) {
        if self.cycle.phase == CyclePhase::Down {
            self.exit_probe_down();
        }

        let cycle = &mut self.cycle;

        cycle.phase = CyclePhase::Refill;
        cycle.phase_start_time = now;
        cycle.rounds_in_phase = 0;

        cycle.is_sample_from_probing = false;
        cycle.last_cycle_stopped_risky_probe = false;

        self.model.clear_bandwidth_lo();
        self.model.clear_inflight_lo();
        cycle.probe_up_rounds = probe_up_rounds;
        cycle.probe_up_acked = 0;
        self.model.restart_round_early();
    }

    fn enter_probe_up(&mut self, now: Instant, cwnd: usize) {
        let cycle = &mut self.cycle;

        cycle.phase = CyclePhase::Up;
        cycle.phase_start_time = now;
        cycle.rounds_in_phase = 0;
        cycle.is_sample_from_probing = true;
        self.raise_inflight_high_slope(cwnd);
        self.model.restart_round_early();
    }

    fn exit_probe_down(&mut self) {
        if !self.cycle.has_advanced_max_bw {
            self.model.advance_max_bandwidth_filter();
            self.cycle.has_advanced_max_bw = true;
        }
    }

    fn update_probe_down(
        &mut self, target_bytes_inflight: usize,
        congestion_event: &BBRv2CongestionEvent, params: &Params,
    ) {
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
            self.cycle.is_sample_from_probing as u128,
            self.model.loss_events_in_round() as u128,
            self.model.get_total_acked_bytes() as u128,
            self.model
                .bandwidth_lo
                .unwrap_or(Bandwidth::infinite())
                .bits_per_second as u128,
            self.model.round_trip_count() as u128,
        ];

        //self.model.write_to_log("DOWN".to_owned(), logging_values);

        if self.cycle.rounds_in_phase == 1 && congestion_event.end_of_round_trip {
            self.cycle.is_sample_from_probing = false;

            if !congestion_event.last_packet_send_state.is_app_limited {
                self.model.advance_max_bandwidth_filter();
                self.cycle.has_advanced_max_bw = true;
            }

            if self.cycle.last_cycle_stopped_risky_probe &&
                !self.cycle.last_cycle_probed_too_high
            {
                self.enter_probe_refill(0, congestion_event.event_time);
                return;
            }
        }

        self.maybe_adapt_upper_bounds(
            target_bytes_inflight,
            congestion_event,
            params,
        );

        if self.is_time_to_probe_bandwidth(
            target_bytes_inflight,
            congestion_event,
            params,
        ) {
            self.enter_probe_refill(0, congestion_event.event_time);
            return;
        }

        // This exit condition is experimental code from Google quiche which
        // diverges from the RFC. Use `disable_probe_down_early_exit` to override
        // the behavior.
        if self.has_stayed_long_enough_in_probe_down(congestion_event, params) {
            self.enter_probe_cruise(congestion_event.event_time);
            return;
        }

        let inflight_with_headroom = self.model.inflight_hi_with_headroom(params);
        let bytes_in_flight = congestion_event.bytes_in_flight;

        if bytes_in_flight > inflight_with_headroom {
            // Stay in PROBE_DOWN.
            return;
        }

        // Transition to PROBE_CRUISE iff we've drained to target.
        let bdp = self.model.bdp0();

        if bytes_in_flight < bdp {
            self.enter_probe_cruise(congestion_event.event_time);
        }
    }

    fn update_probe_cruise(
        &mut self, target_bytes_inflight: usize,
        congestion_event: &BBRv2CongestionEvent, params: &Params,
    ) {
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
            self.cycle.is_sample_from_probing as u128,
            self.model.loss_events_in_round() as u128,
            self.model.get_total_acked_bytes() as u128,
            self.model
                .bandwidth_lo
                .unwrap_or(Bandwidth::infinite())
                .bits_per_second as u128,
            self.model.round_trip_count() as u128,
        ];

        //self.model.write_to_log("CRUISE".to_owned(), logging_values);

        self.maybe_adapt_upper_bounds(
            target_bytes_inflight,
            congestion_event,
            params,
        );

        if self.is_time_to_probe_bandwidth(
            target_bytes_inflight,
            congestion_event,
            params,
        ) {
            self.enter_probe_refill(0, congestion_event.event_time);
        }
    }

    fn update_probe_refill(
        &mut self, target_bytes_inflight: usize,
        congestion_event: &BBRv2CongestionEvent, params: &Params,
    ) {
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
            self.cycle.is_sample_from_probing as u128,
            self.model.loss_events_in_round() as u128,
            self.model.get_total_acked_bytes() as u128,
            self.model
                .bandwidth_lo
                .unwrap_or(Bandwidth::infinite())
                .bits_per_second as u128,
            self.model.round_trip_count() as u128,
        ];

        //self.model.write_to_log("REFILL".to_owned(), logging_values);

        self.maybe_adapt_upper_bounds(
            target_bytes_inflight,
            congestion_event,
            params,
        );

        if self.cycle.rounds_in_phase > 0 && congestion_event.end_of_round_trip {
            self.enter_probe_up(
                congestion_event.event_time,
                congestion_event.prior_cwnd,
            );
        }
    }

    fn update_probe_up(
        &mut self, prior_in_flight: usize, target_bytes_inflight: usize,
        congestion_event: &BBRv2CongestionEvent, params: &Params,
    ) {
        if self.maybe_adapt_upper_bounds(
            target_bytes_inflight,
            congestion_event,
            params,
        ) == AdaptUpperBoundsResult::AdaptedProbedTooHigh
        {
            self.enter_probe_down(
                true,
                false,
                congestion_event.event_time,
                params,
            );
            return;
        }

        self.probe_inflight_high_upward(congestion_event, params);

        let mut is_risky = false;
        let mut is_queuing = false;
        if self.cycle.last_cycle_probed_too_high &&
            prior_in_flight >= self.model.inflight_hi()
        {
            is_risky = true;
        } else if self.cycle.rounds_in_phase > 0 {
            if params.max_probe_up_queue_rounds > 0 {
                if congestion_event.end_of_round_trip {
                    self.model
                        .check_persistent_queue(params.full_bw_threshold, params);
                    if self.model.rounds_with_queueing() >=
                        params.max_probe_up_queue_rounds
                    {
                        is_queuing = true;
                    }
                }
            } else {
                let mut queuing_threshold_extra_bytes =
                    self.model.queueing_threshold_extra_bytes();
                if params.add_ack_height_to_queueing_threshold {
                    queuing_threshold_extra_bytes += self.model.max_ack_height();
                }
                let queuing_threshold = (params.full_bw_threshold *
                    self.model.bdp0() as f32)
                    as usize +
                    queuing_threshold_extra_bytes;

                is_queuing =
                    congestion_event.bytes_in_flight >= queuing_threshold;
            }
        }


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
            self.cycle.is_sample_from_probing as u128,
            self.model.loss_events_in_round() as u128,
            self.model.get_total_acked_bytes() as u128,
            self.model
                .bandwidth_lo
                .unwrap_or(Bandwidth::infinite())
                .bits_per_second as u128,
            self.model.round_trip_count() as u128,
        ];

        //self.model.write_to_log("UP".to_owned(), logging_values);

        if is_risky || is_queuing {
            self.enter_probe_down(
                false,
                is_risky,
                congestion_event.event_time,
                params,
            );
        }
    }

    fn is_time_to_probe_bandwidth(
        &self, target_bytes_inflight: usize,
        congestion_event: &BBRv2CongestionEvent, params: &Params,
    ) -> bool {
        if self.has_cycle_lasted(
            self.cycle.probe_wait_time.unwrap(),
            congestion_event,
        ) {
            return true;
        }

        if self.is_time_to_probe_for_reno_coexistence(
            target_bytes_inflight,
            1.0,
            congestion_event,
            params,
        ) {
            return true;
        }

        false
    }

    fn maybe_adapt_upper_bounds(
        &mut self, target_bytes_inflight: usize,
        congestion_event: &BBRv2CongestionEvent, params: &Params,
    ) -> AdaptUpperBoundsResult {
        println!("in maybe_adapt_upper_bounds");
        let send_state = congestion_event.last_packet_send_state;

        if !send_state.is_valid {
            println!("early return");
            return AdaptUpperBoundsResult::NotAdaptedInvalidSample;
        }

        // TODO(vlad): use BytesInFlight?
        let mut inflight_at_send = send_state.bytes_in_flight;
        if params.use_bytes_delivered_for_inflight_hi {
            inflight_at_send = self.model.total_bytes_acked() -
                congestion_event.last_packet_send_state.total_bytes_acked;
        }
        println!(
            "is sample from probing: {:?}",
            self.cycle.is_sample_from_probing
        );
        if self.cycle.is_sample_from_probing {
            if self.model.is_inflight_too_high(
                congestion_event,
                params.probe_bw_full_loss_count,
                params,
            ) {
                println!("first if check passed");
                self.cycle.is_sample_from_probing = false;
                if !send_state.is_app_limited ||
                    params.max_probe_up_queue_rounds > 0
                {
                    let inflight_target = (target_bytes_inflight as f32 *
                        (1.0 - params.beta))
                        as usize;

                    let mut new_inflight_hi =
                        inflight_at_send.max(inflight_target);

                    if params.limit_inflight_hi_by_max_delivered {
                        new_inflight_hi = self
                            .model
                            .max_bytes_delivered_in_round()
                            .max(new_inflight_hi);
                    }
                    println!(
                        "setting inflight hi from maybe_adapt_upper_bounds 1"
                    );
                    self.model.set_inflight_hi(new_inflight_hi);
                }
                return AdaptUpperBoundsResult::AdaptedProbedTooHigh;
            }
            return AdaptUpperBoundsResult::AdaptedOk;
        }

        if self.model.inflight_hi() == self.model.inflight_hi_default() {
            return AdaptUpperBoundsResult::NotAdaptedInflightHighNotSet;
        }

        // Raise the upper bound for inflight.
        if inflight_at_send > self.model.inflight_hi() {
            println!("setting inflight hi from maybe_adapt_upper_bounds 2");
            self.model.set_inflight_hi(inflight_at_send);
        }

        AdaptUpperBoundsResult::AdaptedOk
    }

    fn has_cycle_lasted(
        &self, duration: Duration, congestion_event: &BBRv2CongestionEvent,
    ) -> bool {
        (congestion_event.event_time - self.cycle.start_time) > duration
    }

    fn has_phase_lasted(
        &self, duration: Duration, congestion_event: &BBRv2CongestionEvent,
    ) -> bool {
        (congestion_event.event_time - self.cycle.phase_start_time) > duration
    }

    fn is_time_to_probe_for_reno_coexistence(
        &self, target_bytes_inflight: usize, probe_wait_fraction: f64,
        _congestion_event: &BBRv2CongestionEvent, params: &Params,
    ) -> bool {
        if !params.enable_reno_coexistence {
            return false;
        }

        let mut rounds = params.probe_bw_probe_max_rounds;
        if params.probe_bw_probe_reno_gain > 0.0 {
            let reno_rounds = (params.probe_bw_probe_reno_gain *
                target_bytes_inflight as f32 /
                DEFAULT_MSS as f32) as usize;
            rounds = reno_rounds.min(rounds);
        }

        self.cycle.rounds_since_probe >=
            (rounds as f64 * probe_wait_fraction) as usize
    }

    // Used to prevent a BBR2 flow from staying in PROBE_DOWN for too
    // long, as seen in some multi-sender simulator tests.
    //
    // This is experimental code from Google quiche and diverges from the RFC. Use
    // `disable_probe_down_early_exit` to override the behavior.
    // - RFC: https://www.ietf.org/archive/id/draft-ietf-ccwg-bbr-02.html#name-probebw_down
    // - Google quiche: https://github.com/google/quiche/blob/b370e7a/quiche/quic/core/congestion_control/bbr2_probe_bw.cc#L142
    fn has_stayed_long_enough_in_probe_down(
        &self, congestion_event: &BBRv2CongestionEvent, params: &Params,
    ) -> bool {
        if params.disable_probe_down_early_exit {
            return false;
        }

        // Stay in PROBE_DOWN for at most the time of a min rtt, as it is done
        // in BBRv1.
        self.has_phase_lasted(self.model.min_rtt(), congestion_event)
    }

    fn raise_inflight_high_slope(&mut self, cwnd: usize) {
        let growth_this_round = 1usize << self.cycle.probe_up_rounds;
        // The number 30 below means `growth_this_round` is capped at 1G and the
        // lower bound of `probe_up_bytes` is (practically) 1 mss, at this
        // speed `inflight_hi`` grows by approximately 1 packet per packet acked.
        self.cycle.probe_up_rounds = self.cycle.probe_up_rounds.add(1).min(30);
        let probe_up_bytes = cwnd / growth_this_round;
        self.cycle.probe_up_bytes = Some(probe_up_bytes.max(DEFAULT_MSS));
    }

    fn probe_inflight_high_upward(
        &mut self, congestion_event: &BBRv2CongestionEvent, params: &Params,
    ) {
        if params.probe_up_ignore_inflight_hi {
            // When inflight_hi is disabled in PROBE_UP, it increases when
            // the number of bytes delivered in a round is larger inflight_hi.
            return;
        } else {
            // TODO(vlad): probe_up_simplify_inflight_hi?
            if congestion_event.prior_bytes_in_flight <
                congestion_event.prior_cwnd
            {
                // Not fully utilizing cwnd, so can't safely grow.
                return;
            }

            if congestion_event.prior_cwnd < self.model.inflight_hi() {
                // Not fully using inflight_hi, so don't grow it.
                return;
            }

            self.cycle.probe_up_acked += congestion_event.bytes_acked;
        }

        if let Some(probe_up_bytes) = self.cycle.probe_up_bytes {
            if self.cycle.probe_up_acked >= probe_up_bytes {
                let delta = self.cycle.probe_up_acked / probe_up_bytes;
                // probe_up_acked is set to the remainder mod probe_up_bytes;
                // probe_up_acked >= delta * probe_up_bytes so this
                // can't underflow.
                self.cycle.probe_up_acked -= delta * probe_up_bytes;
                let new_inflight_hi =
                    self.model.inflight_hi() + delta * DEFAULT_MSS;
                if new_inflight_hi > self.model.inflight_hi() {
                    println!(
                        "setting inflight hi from probe_inflight_high_upward 1"
                    );
                    self.model.set_inflight_hi(new_inflight_hi);
                }
            }
        }

        if congestion_event.end_of_round_trip {
            self.raise_inflight_high_slope(congestion_event.prior_cwnd);
        }
    }

    fn into_probe_rtt(
        mut self, now: Instant, congestion_event: Option<&BBRv2CongestionEvent>,
        params: &Params,
    ) -> Mode {
        self.leave(now, congestion_event);
        let mut next_mode = Mode::probe_rtt(self.model, self.cycle);
        next_mode.enter(now, congestion_event, params);
        next_mode
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::recovery::gcongestion::bbr2::SendTimeState;
    use crate::recovery::gcongestion::bbr2::DEFAULT_PARAMS;
    use crate::recovery::Bandwidth;

    #[test]
    fn plateau_normal_execution() {
        // correct up behavior with concrete observed values
        let last_packet_send_state_normal = SendTimeState {
            is_valid: true,
            is_app_limited: true,
            total_bytes_sent: 12046680,
            total_bytes_acked: 7147216,
            total_bytes_lost: 4270825,
            bytes_in_flight: 4897835,
        };
        let test_event_normal = BBRv2CongestionEvent {
            event_time: Instant::now(),
            prior_cwnd: 6829574 as usize, // should be way higher
            prior_bytes_in_flight: 6829574 as usize, // should be way higher
            bytes_in_flight: 6824174 as usize,
            bytes_acked: 5400 as usize,
            bytes_lost: 0,
            end_of_round_trip: true,
            is_probing_for_bandwidth: true,
            sample_max_bandwidth: Some(Bandwidth::from_bytes_per_second(
                20977504,
            )),
            sample_min_rtt: Some(Duration::from_millis(1867)), // about right
            last_packet_send_state: last_packet_send_state_normal,
        };

        let params = &DEFAULT_PARAMS;
        let mut model = BBRv2NetworkModel::new(
            params,
            Duration::from_millis(333),
            "".to_owned(),
        );

        model.set_total_acked_bytes(7147216); // necessary, as otherwise we have an overflow

        let cycle = Cycle::default();
        let mut probe_bw = ProbeBW { model, cycle };
        // setting this to true should force us to update the inflight_hi
        probe_bw.cycle.is_sample_from_probing = true;
        // probe_bw.model.set_inflight_hi(4738476 as usize); --> this should
        // happen in update_probe_up
        let bdp = probe_bw
            .model
            .bdp1(Bandwidth::from_bytes_per_second(20977504));
        let target_bytes = bdp.min(6829574 as usize);

        probe_bw.update_probe_up(
            test_event_normal.prior_bytes_in_flight as usize,
            target_bytes,
            &test_event_normal,
            params,
        );

        assert_eq!(probe_bw.cycle.probe_up_rounds, 1);
        // Slope is increased at the end of round by decreasing probe_up_bytes.
    }

    #[test]
    fn plateau_plateau_execution() {
        let last_packet_send_state_plateau = SendTimeState {
            is_valid: true,
            is_app_limited: false, /* this should be true in practice, but in
                                    * the test, both should be false */
            total_bytes_sent: 586320,
            total_bytes_acked: 562668,
            total_bytes_lost: 0,
            bytes_in_flight: 21973,
        };
        let test_event_plateau = BBRv2CongestionEvent {
            event_time: Instant::now(),
            prior_cwnd: 21973 as usize, // should be way higher
            prior_bytes_in_flight: 21973 as usize, // should be way higher
            bytes_in_flight: 0,         // real: 0
            bytes_acked: 21973 as usize,
            bytes_lost: 0,
            end_of_round_trip: true,
            is_probing_for_bandwidth: true,
            sample_max_bandwidth: Some(Bandwidth::from_bytes_per_second(174040)), /* should be way higher */
            sample_min_rtt: Some(Duration::from_millis(1009)), // about right
            last_packet_send_state: last_packet_send_state_plateau,
        };

        let params = &DEFAULT_PARAMS;
        let mut model = BBRv2NetworkModel::new(
            params,
            Duration::from_millis(333),
            "".to_owned(),
        );
        model.set_total_acked_bytes(562668);
        let cycle = Cycle::default();
        let mut probe_bw = ProbeBW { model, cycle };
        probe_bw.cycle.is_sample_from_probing = true;

        // this is simply the initial maximum value --> thus we return early in
        // probe_inflight_high_upward (which is called in update_probe_up), which
        // is why the round counter is not increased (would be at the end of the
        // function)
        // this controls the upper limit of how the recent rounds and _should_ be
        // updated once per round
        probe_bw
            .model
            .set_inflight_hi(1.84467440737096E+019 as usize);

        assert_eq!(probe_bw.cycle.probe_up_rounds, 0);

        let bdp = probe_bw
            .model
            .bdp1(Bandwidth::from_bytes_per_second(174040));
        let target_bytes = bdp.min(21973 as usize);

        // main method; this calls probe_inflight_high_upward, which returns too
        // early due to the maximum inflight_hi
        probe_bw.update_probe_up(
            test_event_plateau.prior_bytes_in_flight as usize,
            target_bytes,
            &test_event_plateau,
            params,
        );
        // check that returns us too early
        assert_eq!(
            test_event_plateau.prior_cwnd < probe_bw.model.inflight_hi(),
            true
        );
        assert_eq!(probe_bw.cycle.probe_up_rounds, 0);
    }

    #[test]
    fn set_inflight_hi_correct() {
        // data obtained experimentally
        let is_risky = false;
        let is_queuing = false;
        let model_rounds_with_queueing = 0;
        let model_min_bytes_in_flight_in_round = 7047620;
        let model_cwnd_gain = 2;
        let model_pacing_gain = 1;
        let model_inflight_hi = usize::MAX; // should be then set to 7535548
        let congestion_event_event_time_milis = 0;
        let congestion_event_prior_cwnd = 9501470;
        let congestion_event_prior_bytes_in_flight = 7058420;
        let congestion_event_bytes_in_flight = 7047620;
        let congestion_event_bytes_acked = 1350;
        let congestion_event_bytes_lost = 9450;
        let congestion_event_end_of_round_trip = false;
        let congestion_event_probing_for_bw = true;
        let max_bw = 21482635;
        let min_rtt = 2749;
        let send_state_is_valid = true;
        let send_state_is_app_limited = true; // will be false in the next step
        let send_state_total_bytes_sent = 191059942;
        let send_state_total_bytes_acked = 180688585;
        let send_state_total_bytes_lost = 906470;
        let send_state_bytes_in_flight = 9463260;
        let cycle_is_sample_from_probing = true;
        let model_loss_events_in_round = 10; // will be reset to 0 afterwards

        let model_inflight_hi_correct = 7535548;

        let last_packet_send_state = SendTimeState {
            is_valid: send_state_is_valid,
            is_app_limited: send_state_is_app_limited,
            total_bytes_sent: send_state_total_bytes_sent,
            total_bytes_acked: send_state_total_bytes_acked,
            total_bytes_lost: send_state_total_bytes_lost,
            bytes_in_flight: send_state_bytes_in_flight,
        };
        let congestion_event = BBRv2CongestionEvent {
            event_time: Instant::now(),
            prior_cwnd: congestion_event_prior_cwnd as usize,
            prior_bytes_in_flight: congestion_event_prior_bytes_in_flight
                as usize,
            bytes_in_flight: congestion_event_bytes_in_flight,
            bytes_acked: congestion_event_bytes_acked as usize,
            bytes_lost: congestion_event_bytes_lost,
            end_of_round_trip: congestion_event_end_of_round_trip,
            is_probing_for_bandwidth: congestion_event_probing_for_bw,
            sample_max_bandwidth: Some(Bandwidth::from_bytes_per_second(max_bw)),
            sample_min_rtt: Some(Duration::from_millis(min_rtt)),
            last_packet_send_state,
        };

        let params = &DEFAULT_PARAMS;
        let mut model = BBRv2NetworkModel::new(
            params,
            Duration::from_millis(333),
            "".to_owned(),
        );
        model.set_total_acked_bytes(562668); // update to avoid overflow
        model.set_loss_events_in_round(model_loss_events_in_round);
        model.set_inflight_hi(model_inflight_hi);
        let cycle = Cycle::default();
        let mut probe_bw = ProbeBW { model, cycle };
        probe_bw.cycle.is_sample_from_probing = cycle_is_sample_from_probing;

        let bdp = probe_bw
            .model
            .bdp1(Bandwidth::from_bytes_per_second(174040));
        let target_bytes = bdp.min(21973 as usize);

        // main method; this calls probe_inflight_high_upward, which returns too
        // early due to the maximum inflight_hi
        probe_bw.update_probe_up(
            congestion_event.prior_bytes_in_flight as usize,
            target_bytes,
            &congestion_event,
            params,
        );
        // check that returns us too early
        assert_eq!(
            congestion_event.prior_cwnd < probe_bw.model.inflight_hi(),
            true
        );
    }
    #[rstest]
    fn probe_upward(#[values(100, 10_000, 65_536, 300_000)] step: usize) {
        let test_event =
            |probe_bw: &ProbeBW, bytes_acked: usize, end_of_round_trip: bool| {
                BBRv2CongestionEvent {
                    event_time: probe_bw.cycle.start_time,
                    prior_cwnd: probe_bw.model.inflight_hi(),
                    prior_bytes_in_flight: probe_bw.model.inflight_hi(),
                    bytes_in_flight: 0,
                    bytes_acked,
                    bytes_lost: 0,
                    end_of_round_trip,
                    is_probing_for_bandwidth: true,
                    sample_max_bandwidth: None,
                    sample_min_rtt: None,
                    last_packet_send_state: SendTimeState::default(),
                }
            };

        let do_probe_up = |probe_bw: &mut ProbeBW,
                           params: &Params,
                           total_bytes: usize| {
            let mut remaining = total_bytes;
            loop {
                let congestion_event =
                    test_event(probe_bw, step.min(remaining), false);

                probe_bw.probe_inflight_high_upward(&congestion_event, params);

                if remaining > step {
                    remaining -= step;
                } else {
                    break;
                }
            }
        };

        let params = &DEFAULT_PARAMS;
        let model = BBRv2NetworkModel::new(
            params,
            Duration::from_millis(333),
            "".to_owned(),
        );
        let cycle = Cycle::default();
        let mut probe_bw = ProbeBW { model, cycle };
        probe_bw.model.set_inflight_hi(100_000);
        probe_bw.raise_inflight_high_slope(100_000);
        assert_eq!(probe_bw.cycle.probe_up_rounds, 1);
        assert_eq!(probe_bw.cycle.probe_up_bytes, Some(100_000));

        assert_eq!(probe_bw.model.inflight_hi(), 100_000);
        do_probe_up(&mut probe_bw, params, 1_000_000);
        // End inflight_hi should be independent of step size.
        assert_eq!(probe_bw.model.inflight_hi(), 113_000);

        // Slope is increased at the end of round by decreasing probe_up_bytes.
        probe_bw.probe_inflight_high_upward(
            &test_event(&probe_bw, 10_000, true),
            params,
        );
        assert_eq!(probe_bw.cycle.probe_up_rounds, 2);
        assert_eq!(probe_bw.cycle.probe_up_bytes, Some(56500));

        do_probe_up(&mut probe_bw, params, 1_000_000);
        // End inflight_hi should be independent of step size.
        assert_eq!(probe_bw.model.inflight_hi(), 135_100);

        probe_bw.probe_inflight_high_upward(
            &test_event(&probe_bw, 10_000, true),
            params,
        );
        assert_eq!(probe_bw.cycle.probe_up_rounds, 3);
        assert_eq!(probe_bw.cycle.probe_up_bytes, Some(33775));

        do_probe_up(&mut probe_bw, params, 1_000_000);
        // End inflight_hi should be independent of step size.
        assert_eq!(probe_bw.model.inflight_hi(), 174_100);
    }
}
