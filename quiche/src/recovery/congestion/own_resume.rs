// Based on: https://github.com/ana-cc/quiche/blob/resume_latest/quiche/src/recovery/congestion/resume.rs (11.08.2025)

use crate::recovery::congestion::Acked;
use std::{
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    u64,
};
//write back saved cc params to file
use std::fs;
use std::path::Path;

const SAVED_CC_FILE: &str = "saved_params.csv";
const PARAMS_MAXIMUM_GAP: Duration = Duration::from_secs(120 * 60);

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
    in_state_timer: Instant,
    trace_id: String,
    enabled: bool,
    cr_state: CrState,
    saved_rtt: Duration,
    saved_cwnd: usize,
    pipesize: usize,
    jump_cwnd: usize,
    pub total_acked: usize,
    cwnd: usize,
    rtt: Option<Duration>,
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
    pub fn new(trace_id: &str, file_name: &str) -> Self {
        // enabled will become false if either of the required CR ENV VARS is not supplied
        let mut enabled = true;
        let mut saved_rtt = Duration::from_secs(u64::MAX);

        let mut saved_cwnd = 0;
        let mut saved_time = Duration::ZERO;
        if Path::new(SAVED_CC_FILE).exists() {
            let file_contents = fs::read_to_string(file_name).unwrap();

            let file_array: Vec<&str> = file_contents.split(',').collect();
            if file_array.len() > 1 {
                let rtt_string = file_array[1];
                if let Ok(rtt_int) = rtt_string.parse::<u64>() {
                    saved_rtt = Duration::from_secs(rtt_int.try_into().unwrap());
                    println!("Found saved rtt! {:?}", saved_rtt);
                } else {
                    println!("Didnt find rtt");
                }

                let cwnd_string = file_array[3];
                if let Ok(cwnd_int) = cwnd_string.parse::<usize>() {
                    saved_cwnd = cwnd_int;
                    println!("Found saved cwnd! {:?}", saved_cwnd);
                } else {
                    println!("Didnt find cwnd");
                }

                let time_string = file_array[5];
                if let Ok(time_int) = time_string.parse::<u64>() {
                    saved_time =
                        Duration::from_secs(time_int.try_into().unwrap());
                    println!("Found saved time! {:?}", saved_time);
                } else {
                    println!("Didnt find time");
                }
                let current_time =
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
                if current_time - saved_time > PARAMS_MAXIMUM_GAP {
                    //abort
                    enabled = false;
                }
            } else {
                enabled = false;
            }
        } else {
            enabled = false;
        }

        Self {
            in_state_timer: Instant::now(),
            trace_id: trace_id.to_string(),
            enabled,
            cr_state: CrState::default(),
            saved_rtt,
            saved_cwnd,
            jump_cwnd: 0,
            pipesize: 0,
            total_acked: 0,
            rtt: Some(Duration::ZERO),
            cwnd: 0,
        }
    }

    pub fn enabled(&mut self) -> bool {
        if self.enabled {
            self.cr_state != CrState::Normal
            //true
        } else {
            println!("not enabled");
            if self.cr_state != CrState::Normal {
                self.change_state(CrState::Normal);
            }

            false
        }
    }
    pub fn get_state(&self) -> CrState {
        self.cr_state
    }

    pub fn get_saved_rtt(&self) -> u64 {
        self.saved_rtt.as_secs() as u64
    }

    pub fn get_saved_cwnd(&self) -> f64 {
        self.saved_cwnd as f64
    }

    #[inline]
    pub fn change_state(&mut self, state: CrState) {
        self.cr_state = state;
    }
    pub fn get_jump_cwnd(&self) -> usize {
        self.jump_cwnd
    }

    pub fn set_saved_rtt(&mut self, new_rtt: u64) {
        self.saved_rtt = Duration::from_secs(new_rtt)
    }

    pub fn get_state_timer(&self) -> Instant {
        self.in_state_timer
    }
    // Returns (new_cwnd, new_ssthresh), both optional
    pub fn process_ack(
        &mut self, largest_pkt_sent: u64, packet: &Acked, flightsize: usize,
        _iw_acked: bool,
    ) -> (Option<usize>, Option<usize>) {
        println!("in process ack!!");
        self.total_acked += 1; // this was used by the other implementation: packet.size; but doesnt make too much sense here: after all the iw is saved in packets not bytes
        match self.cr_state {
            CrState::Unvalidated(first_packet) => {
                self.pipesize += packet.size;
                if packet.pkt_num >= first_packet {
                    if flightsize <= self.pipesize {
                        trace!("{} careful resume complete", self.trace_id);
                        self.change_state(
                            CrState::Normal,
                            // CarefulResumeTrigger::LastUnvalidatedPacketAcknowledged,
                        );
                        (Some(self.pipesize), None)
                    } else {
                        trace!(
                            "{} entering careful resume validating phase",
                            self.trace_id
                        );
                        // Store the last packet number that was sent in the Unvalidated Phase
                        self.change_state(
                            CrState::Validating(largest_pkt_sent),
                            // CarefulResumeTrigger::FirstUnvalidatedPacketAcknowledged,
                        );
                        (Some(flightsize), None)
                    }
                } else {
                    (None, None)
                }
            },
            CrState::Validating(last_packet) => {
                self.pipesize += packet.size;
                if packet.pkt_num >= last_packet {
                    trace!("{} careful resume complete", self.trace_id);
                    self.change_state(
                        CrState::Normal,
                        //CarefulResumeTrigger::LastUnvalidatedPacketAcknowledged,
                    );
                }
                (None, None)
            },
            CrState::SafeRetreat(last_packet) => {
                if packet.pkt_num >= last_packet {
                    trace!("{} careful resume complete", self.trace_id);
                    self.change_state(
                        CrState::Normal,
                        //CarefulResumeTrigger::ExitRecovery,
                    );
                    (None, Some(self.pipesize))
                } else {
                    self.pipesize += packet.size;
                    (None, None)
                }
            },
            _ => (None, None),
        }
    }

    //returns cwnd
    pub fn send_packet(
        &mut self, rtt_sample: Option<Duration>, cwnd: usize,
        largest_pkt_sent: u64, app_limited: bool, iw_acked: bool,
    ) -> usize {
        self.cwnd = cwnd;
        self.rtt = rtt_sample;
        // Do nothing when data limited to avoid having insufficient data
        // to be able to validate transmission at a higher rate
        if app_limited {
            return 0; //self.saved_cwnd;
        }
        if !iw_acked {
            return 0; //self.saved_cwnd;
        }
        match self.cr_state {
            CrState::Reconnaissance => {
                self.jump_cwnd = self.saved_cwnd / 2;
                //self.jump_cwnd = cmp::max(MAX_JUMP, self.saved_cwnd / 2); //--> this _would_ be correct following the draft, but it adds roughly 5s to flow completion?
                println!("-----------jump is: {:?}----------", self.jump_cwnd);
                if self.jump_cwnd == 0 {
                    self.change_state(CrState::Normal);
                    return 0;
                }
                //check rtt in recon: path changed or rtt too small?
                let current_rtt = match rtt_sample {
                    Some(s) => s,
                    None => {
                        // Don't make any decisions until we have an RTT sample
                        return cwnd;
                    },
                };
                // Confirm RTT is similar to that of the saved connection
                if current_rtt <= self.saved_rtt / 2
                    || current_rtt >= self.saved_rtt * 10
                // this is arbitrary, but seems to make somewhat sense
                {
                    println!(
                    "{} current RTT too divergent from saved RTT - not using careful resume; \
                    rtt_sample={:?} saved_rtt={:?}",
                    self.trace_id, current_rtt, self.saved_rtt
                );
                    self.change_state(CrState::Normal);
                }
                self.change_state(CrState::Unvalidated(largest_pkt_sent));
                self.pipesize = cwnd;
                return self.jump_cwnd;
            },

            _ => return 0,
        }
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
            CrState::Validating(_) => {
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
