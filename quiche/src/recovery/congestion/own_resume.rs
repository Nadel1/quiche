// Based on: https://github.com/ana-cc/quiche/blob/resume_latest/quiche/src/recovery/congestion/resume.rs (11.08.2025)

use crate::recovery::congestion::{Acked, Congestion};
use std::{cmp, f32::MIN, time::{Duration, Instant}};

const CR_EVENT_MAXIMUM_GAP: Duration = Duration::from_secs(60);
const MAX_JUMP:usize=2000;//configured max cwnd

// No observe state as that always applies to the saved connection and never the current connection
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub enum CrState {
    #[default]
    Reconnaissance,
    // The next two states store the first packet sent when entering that state
    Unvalidated(u64),
    Validating(u64),
    // Stores the last packet sent during the Unvalidated Phase
    SafeRetreat(u64),
    Normal,
}
//TODO: add deleted qlog metrics back in
pub struct OwnResume {
    trace_id: String,
    enabled: bool,
    cr_state: CrState,
    saved_rtt: Duration,
    saved_cwnd: usize,
    pipesize: usize,
    jump_cwnd:usize,
    pub total_acked: usize,
}

impl std::fmt::Debug for OwnResume {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "cr_state={:?} ", self.cr_state)?;
        write!(f, "saved_rtt={:?} ", self.saved_rtt)?;
        write!(f, "saved_cwnd={:?} ", self.saved_cwnd)?;
        write!(f, "pipesize={:?} ", self.pipesize)?;

        Ok(())
    }
}

impl OwnResume {
    pub fn new(trace_id: &str) -> Self {
        // enabled will become false if either of the required CR ENV VARS is not supplied
        let mut enabled = true;
        let mut saved_rtt = Duration::ZERO;
        let mut saved_cwnd = 0;

        if let Some(jw_oss) = std::env::var_os("SAVED_CWND_BYTES") {
            println!("Found saved cwnd bytes!");
            if let Ok(jw_string) = jw_oss.into_string() {
                if let Ok(jw_int) = jw_string.parse::<usize>() {
                    saved_cwnd = jw_int;
                }
            }
        } else {
            println!("Didnt find saved cwnd bytes!");
            enabled = false;
        }

        if let Some(rtt_oss) = std::env::var_os("SAVED_RTT") {
            println!("Found saved rtt!");
            if let Ok(rtt_string) = rtt_oss.into_string() {
                if let Ok(rtt_int) = rtt_string.parse::<usize>() {
                    saved_rtt =
                        Duration::from_millis(rtt_int.try_into().unwrap());
                }
            }
        } else {
            println!("Didnt find saved rtt!");
            enabled = false;
        }

        if let Some(no_sr_oss) = std::env::var_os("DISABLE_SR") {
            println!("Found disable sr!");
        }

        Self {
            trace_id: trace_id.to_string(),
            enabled,
            cr_state: CrState::default(),
            saved_rtt,
            saved_cwnd,
            jump_cwnd:0,
            pipesize: 0,
            total_acked: 0,
        }
    }

    pub fn setup(&mut self, saved_rtt: Duration, saved_cwnd: usize) {
        self.enabled = true;
        self.saved_rtt = saved_rtt;
        self.saved_cwnd = saved_cwnd;
        println!("{} careful resume configured", self.trace_id);
    }

    pub fn enabled(&self) -> bool {
        println!("In enabled! cr state is {:?}",self.cr_state);
        if self.enabled {
            self.cr_state != CrState::Normal
        } else {
            false
        }
    }
    pub fn get_state(&self) -> CrState {
        self.cr_state
    }

    pub fn get_saved_cwnd(&self) -> f64 {
        self.saved_cwnd as f64
    }

    #[inline]
    fn change_state(&mut self, state: CrState) {
        self.cr_state = state;
        
    }
    pub fn get_jump_cwnd(&self)->usize{
        self.jump_cwnd
    }

    // Returns (new_cwnd, new_ssthresh), both optional
    pub fn process_ack(
        &mut self, largest_pkt_sent: u64, packet: &Acked, flightsize: usize,iw_acked: bool
    ) -> (Option<usize>, Option<usize>) {
        println!("in process ack!!");
        self.total_acked += packet.size;
        match self.cr_state {
            CrState::Reconnaissance=>{
                if iw_acked{
                    self.change_state(CrState::Unvalidated(largest_pkt_sent));
                    self.pipesize=flightsize;//initialise the pipesize to the flightsize
                    self.jump_cwnd=cmp::min(MAX_JUMP,(self.saved_cwnd/2));
                    //cwnd=jump_cwnd ?how do i set this??
                }
                (None,None)
            }
            CrState::Unvalidated(first_packet) => {
                println!("in unvalidated phase!");
                self.pipesize += packet.size;
                if packet.pkt_num >= first_packet {
                    if flightsize <= self.pipesize {
                        println!("{} careful resume complete", self.trace_id);
                        self.change_state(CrState::Normal);
                        (Some(self.pipesize), None)
                    } else {
                        println!(
                            "{} entering careful resume validating phase",
                            self.trace_id
                        );
                        // Store the last packet number that was sent in the Unvalidated Phase
                        self.change_state(CrState::Validating(largest_pkt_sent));
                        (Some(flightsize), None)
                    }
                } else {
                    (None, None)
                }
            },
            CrState::Validating(last_packet) => {
                println!("in validating phase!");
                self.pipesize += packet.size;
                if packet.pkt_num >= last_packet {
                    println!("{} careful resume complete", self.trace_id);
                    self.change_state(CrState::Normal);
                }
                (None, None)
            },
            CrState::SafeRetreat(last_packet) => {
                println!("in safe retreat phase!");
                if packet.pkt_num >= last_packet {
                    println!("{} careful resume complete", self.trace_id);
                    self.change_state(CrState::Normal);
                    (None, Some(self.pipesize))
                } else {
                    self.pipesize += packet.size;
                    (None, None)
                }
            },
            _ => (None, None),
        }
    }

    pub fn send_packet(
        &mut self, rtt_sample: Option<Duration>, cwnd: usize,
        largest_pkt_sent: u64, app_limited: bool, iw_acked: bool
    ) -> usize {
        println!("in send packet!!");
        // Do nothing when data limited to avoid having insufficient data
        // to be able to validate transmission at a higher rate
        if app_limited {
            return 0;
        }
        if !iw_acked {
            return 0;
        }
        match self.cr_state {
            CrState::Unvalidated(largest_packet)=>{
                //Pacing ... somehow
                let now = Instant::now();
                //congestion.set_pacing_rate(rtt_sample.unwrap().as_secs(), now);
            }
            _ => return 0
        }
        //else if  self.cr_state == CrState::Reconnaissance {//meaning iw is acked and we are in the recon phase --> go to unvalidated phase
        //    println!("-----Set jump in send_packet in resume-----");
        //    let jump = (self.saved_cwnd / 2).saturating_sub(cwnd);// this should be done on entry to unvalidated phase
//
        //    if jump == 0 {
        //        self.change_state(CrState::Normal);
        //        return 0;
        //    }
//
        //    let current_rtt = match rtt_sample {
        //        Some(s) => s,
        //        None => {
        //            // Don't make any decisions until we have an RTT sample
        //            return 0;
        //        },
        //    };
//
        //    // Confirm RTT is similar to that of the saved connection
        //    if current_rtt <= self.saved_rtt / 2
        //        || current_rtt >= self.saved_rtt * 10
        //    {
        //        println!(
        //            "{} current RTT too divergent from saved RTT - not using careful resume; \
        //            rtt_sample={:?} saved_rtt={:?}",
        //            self.trace_id, current_rtt, self.saved_rtt
        //        );
        //        self.change_state(CrState::Normal);
        //        return 0;
        //    }
//
        //    // Store the first packet number that was sent in the Unvalidated Phase
        //    println!(
        //        "{} entering careful resume unvalidated phase",
        //        self.trace_id
        //    );
        //    self.change_state(CrState::Unvalidated(largest_pkt_sent));
        //    self.pipesize = cwnd;
        //    // we return the jump in window, CC code handles the increase in cwnd
        //    //return jump;
        //}

        0
    }

    pub fn congestion_event(&mut self, largest_pkt_sent: u64) -> usize {
        println!("in congestion event!!");
        match self.cr_state {
            CrState::Unvalidated(_) => {
                println!("{} congestion during unvalidated phase", self.trace_id);

                // TODO: mark used CR parameters as invalid for future connections

                //if self.use_sr {
                //    self.change_state(
                //        CrState::SafeRetreat(largest_pkt_sent),
                //        CarefulResumeTrigger::PacketLoss,
                //    );
                //    self.pipesize / 2

                self.change_state(CrState::SafeRetreat(largest_pkt_sent));
                0
            },
            CrState::Validating(p) => {
                println!("{} congestion during validating phase", self.trace_id);

                // TODO: mark used CR parameters as invalid for future connections

                //if self.use_sr {
                //    self.change_state(
                //        CrState::SafeRetreat(p),
                //        CarefulResumeTrigger::PacketLoss,
                //    );
                //    self.pipesize / 2

                self.change_state(CrState::Normal);
                0
            },
            CrState::Reconnaissance => {
                println!("-----{} congestion during reconnaissance - abandoning careful resume-----", self.trace_id);

                self.change_state(CrState::Normal);
                0
            },
            _ => 0,
        }
    }
}

pub struct CRMetrics {
    trace_id: String,
    iw: usize,
    min_rtt: Duration,
    cwnd: usize,
    last_update: Instant,
}

impl CRMetrics {
    pub fn new(trace_id: &str, iw: usize) -> Self {
        Self {
            trace_id: trace_id.to_string(),
            iw,
            min_rtt: Duration::ZERO,
            cwnd: 0,
            last_update: Instant::now(),
        }
    }

    // Implementation of the CR observe phase
    pub fn maybe_update(
        &mut self, new_min_rtt: Duration, new_cwnd: usize,
    ) -> Option<CREvent> {
        // Initial guess at something that might work, needs further research
        let now = Instant::now();
        let time_since_last_update = now - self.last_update;

        let should_update = if new_cwnd < self.iw * 4 {
            false
        } else if time_since_last_update > CR_EVENT_MAXIMUM_GAP {
            true
        } else {
            let secs_since_last_update = time_since_last_update.as_secs_f64();
            if secs_since_last_update == 0.0 {
                false
            } else {
                let range = 1.0f64 / secs_since_last_update;

                let min_rtt_micros = self.min_rtt.as_micros() as f64;
                let min_rtt_range_spread = min_rtt_micros * range;
                let min_rtt_range_min = min_rtt_micros - min_rtt_range_spread;
                let min_rtt_range_max = min_rtt_micros + min_rtt_range_spread;

                let cwnd = self.cwnd as f64;
                let cwnd_range_spread = cwnd * range;
                let cwnd_range_min = cwnd - cwnd_range_spread;
                let cwnd_range_max = cwnd + cwnd_range_spread;

                let new_min_rtt_micros = new_min_rtt.as_micros() as f64;
                let new_cwnd_float = new_cwnd as f64;

                new_min_rtt_micros < min_rtt_range_min
                    || new_min_rtt_micros > min_rtt_range_max
                    || new_cwnd_float < cwnd_range_min
                    || new_cwnd_float > cwnd_range_max
            }
        };

        println!(
            "{} maybe_update(new_min_rtt={:?}, new_cwnd={}); updating={}",
            self.trace_id,
            new_min_rtt,
            new_cwnd,
            should_update
        );

        if should_update {
            self.min_rtt = new_min_rtt;
            self.cwnd = new_cwnd;
            self.last_update = now;

            Some(CREvent {
                cwnd: new_cwnd,
                min_rtt: new_min_rtt,
            })
        } else {
            None
        }
    }
}

/// An update in Careful OwnResume observed parameters to be stored/transmitted for future connections
#[derive(Clone, Copy, Debug)]
pub struct CREvent {
    /// A windowed minimum round-trip-time observation
    pub min_rtt: Duration,
    /// The current congestion window, in bytes
    pub cwnd: usize,
}
