// Based on: https://github.com/ana-cc/quiche/blob/resume_latest/quiche/src/recovery/congestion/resume.rs (11.08.2025)

use crate::recovery::congestion::Acked;
use std::{
    cmp,
    fs::{read_to_string, File},
    io::{Read, Write},
    time::{Duration, Instant},
};
//write back saved cc params to file
use std::fs;
use std::path::Path;

const SAVED_CC_FILE: &str = "saved_params.csv";
const CR_EVENT_MAXIMUM_GAP: Duration = Duration::from_secs(60);
const MAX_JUMP: usize = 2000; //configured max cwnd

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
    jump_cwnd: usize,
    pub total_acked: usize,
    time_in_state: Instant, //make sure we dont stay in unvalidated phase longer than one rtt
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
        let mut saved_rtt = Duration::ZERO;

        let mut saved_cwnd = 0;
        if Path::new(SAVED_CC_FILE).exists() {
            let file_contents = fs::read_to_string(file_name).unwrap();
            println!("info.txt content =\n{file_contents}");
            let file_array: Vec<&str> = file_contents.split(',').collect();
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
        } else {
            enabled = false;
        }

        Self {
            time_in_state: Instant::now(),
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

    pub fn setup(&mut self, saved_rtt: Duration, saved_cwnd: usize) {
        self.enabled = true;
        self.saved_rtt = saved_rtt;
        self.saved_cwnd = saved_cwnd;
        println!("{} careful resume configured", self.trace_id);
    }

    pub fn enabled(&mut self) -> bool {
        println!("In enabled! cr state is {:?}", self.cr_state);
        if self.enabled {
            println!("is enabled");
            self.cr_state != CrState::Normal
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
    pub fn get_pipesize(&self) -> usize {
        self.pipesize
    }

    pub fn get_saved_rtt(&self) -> u64 {
        self.saved_rtt.as_secs() as u64
    }

    pub fn get_saved_cwnd(&self) -> f64 {
        self.saved_cwnd as f64
    }

    #[inline]
    fn change_state(&mut self, state: CrState) {
        self.cr_state = state;
    }
    pub fn get_jump_cwnd(&self) -> usize {
        self.jump_cwnd
    }

    fn update_state_timer(&mut self) {
        self.time_in_state = Instant::now()
    }
    // Returns (new_cwnd, new_ssthresh), both optional
    pub fn process_ack(
        &mut self, largest_pkt_sent: u64, packet: &Acked, flightsize: usize,
        iw_acked: bool,
    ) -> (Option<usize>, Option<usize>) {
        println!("in process ack!!");
        self.total_acked += packet.size;
        match self.cr_state {
            CrState::Reconnaissance => {
                if iw_acked {
                    self.update_state_timer();
                    self.change_state(CrState::Unvalidated(largest_pkt_sent));
                    self.pipesize = flightsize; //initialise the pipesize to the flightsize
                    self.jump_cwnd = cmp::min(MAX_JUMP, self.saved_cwnd / 2);
                    println!("---------------set the max jump_cwnd to {:?}-------------",self.jump_cwnd);
                    //cwnd=jump_cwnd ?how do i set this??
                }
                (None, None)
            },
            CrState::Unvalidated(first_packet) => {
                println!("in unvalidated phase!");
                //check that we leave unvalidated phase after 1 rtt
                let now = Instant::now();
                if now - self.time_in_state > self.saved_rtt {
                    self.change_state(CrState::Validating(largest_pkt_sent));
                }
                self.pipesize += packet.size;

                if packet.pkt_num >= first_packet {
                    if flightsize <= self.pipesize {
                        println!("{} careful resume complete", self.trace_id);
                        self.change_state(CrState::Normal);
                        (Some(self.pipesize), None)
                    } else {
                        //received ack for unvalidated packet
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

    //returns cwnd
    pub fn send_packet(
        &mut self, rtt_sample: Option<Duration>, cwnd: usize,
        _largest_pkt_sent: u64, app_limited: bool, iw_acked: bool,
    ) -> usize {
        println!(
            "in send packet!! app limited is {}, iw_acked is {}",
            app_limited, iw_acked
        );
        self.cwnd = cwnd;
        self.rtt = rtt_sample;
        // Do nothing when data limited to avoid having insufficient data
        // to be able to validate transmission at a higher rate
        if app_limited {
            return self.saved_cwnd;
        }
        if !iw_acked {
            return self.saved_cwnd;
        }
        match self.cr_state {
            CrState::Reconnaissance => {
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
                    return cwnd;
                }
            },
            CrState::Unvalidated(_) => {
                return self.get_jump_cwnd(); //sets the cwnd to jump cwnd
            },
            CrState::SafeRetreat(_) => {
                return self.get_pipesize() / 2;
            },
            _ => return cwnd,
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
            self.trace_id, new_min_rtt, new_cwnd, should_update
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
