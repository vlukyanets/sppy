//! Turns VLC's polled status into `PlayerEvent`s. Pure: time is passed in.
//!
//! VLC updates the input time in steps of about 250 ms (sometimes 500 ms), but at the step
//! the value is within ~15 ms of the real position. The tracker anchors at the step it
//! observes and extrapolates between steps. A paused input keeps the time of the last
//! step, so the position at the pause is the extrapolated one.

use super::wire::{InputState, Status, ppm_to_speed};
use crate::PlayerEvent;

/// No input this long after a load (and none seen) means VLC could not open the media.
pub const LOAD_FAIL_MS: u64 = 1500;
/// An input that goes away this close to its end has ended.
pub const END_WINDOW_MS: u64 = 2000;
/// Extrapolation stops this far past the last step (VLC stalls, or a step is late).
pub const MAX_EXTRAPOLATE_MS: u64 = 750;
/// A step this far from the prediction is a seek made in VLC.
pub const USER_SEEK_MS: u64 = 1000;
/// While paused, a time change this large is a seek made in VLC.
pub const PAUSED_JUMP_MS: u64 = 300;
/// The cache must stay below full this long to count as buffering.
pub const BUFFERING_AFTER_MS: u64 = 500;
/// A status this close to a seek target answers that seek.
pub const SEEK_MATCH_MS: u64 = 1000;
/// Steps remembered to undo the polling delay (about 3 s of playback).
const STEP_WINDOW: usize = 12;
/// A seek VLC never shows is forgotten after this long.
pub const SEEK_FORGET_MS: u64 = 3000;
/// The rate VLC reports for a speed we asked for differs by its rounding at most this much.
const RATE_MATCH: f64 = 0.002;

/// What the backend has to do after an observation.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Event(PlayerEvent),
    /// The input is open: add the subtitles of the current item.
    AddSubtitles,
    /// The input went away in the middle (VLC's Stop button): open the item again.
    Reload,
    /// The item opened again: put it back where it was.
    Restore {
        position_ms: u64,
        paused: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Restore {
    position_ms: u64,
    paused: bool,
    /// The reload answers a seek of the caller, who waits for `Seeked`.
    report_seek: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Idle,
    Loading { since_ms: u64, restore: Option<Restore> },
    Open,
    Ended,
    Failed,
}

#[derive(Debug, Clone, Copy)]
struct PendingSeek {
    target_ms: u64,
    sent_ms: u64,
    /// Our own correction after a reload: the caller does not expect `Seeked`.
    internal: bool,
}

#[derive(Debug, Clone)]
pub struct Tracker {
    phase: Phase,
    paused: bool,
    rate_ppm: u64,
    requested_speed: Option<f64>,
    /// VLC's time at its last step.
    raw_ms: u64,
    /// Position at `base_at_ms`; extrapolated from there while playing and no step is known.
    base_ms: u64,
    base_at_ms: u64,
    /// Steps since the last discontinuity: (when noticed, VLC's time). A step is noticed at
    /// the next poll, never before it happened, so the one predicting the furthest
    /// position was noticed soonest.
    steps: std::collections::VecDeque<(u64, u64)>,
    length_ms: Option<u64>,
    pending_seek: Option<PendingSeek>,
    /// The pause state we set ourselves and VLC has not shown yet, with the time we set it.
    expected_pause: Option<(bool, u64)>,
    cache_low_since: Option<u64>,
    buffering: bool,
    last_reported: Option<u64>,
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker {
            phase: Phase::Idle,
            paused: true,
            rate_ppm: 1_000_000,
            requested_speed: None,
            raw_ms: 0,
            base_ms: 0,
            base_at_ms: 0,
            steps: std::collections::VecDeque::new(),
            length_ms: None,
            pending_seek: None,
            expected_pause: None,
            cache_low_since: None,
            buffering: false,
            last_reported: None,
        }
    }
}

impl Tracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// A new item was sent to VLC.
    pub fn load_sent(&mut self, now_ms: u64) {
        let requested_speed = self.requested_speed;
        *self = Tracker {
            phase: Phase::Loading { since_ms: now_ms, restore: None },
            requested_speed,
            ..Tracker::default()
        };
    }

    /// A seek was sent. Returns `true` when the item has ended and VLC has nothing open:
    /// the backend has to load the item again, and the tracker seeks once it is open.
    pub fn seek_sent(&mut self, target_ms: u64, now_ms: u64) -> bool {
        if self.phase == Phase::Ended {
            self.phase = Phase::Loading {
                since_ms: now_ms,
                restore: Some(Restore {
                    position_ms: target_ms,
                    paused: self.paused,
                    report_seek: true,
                }),
            };
            return true;
        }
        self.pending_seek = Some(PendingSeek { target_ms, sent_ms: now_ms, internal: false });
        false
    }

    pub fn speed_sent(&mut self, speed: f64) {
        self.requested_speed = Some(speed);
    }

    /// The current position estimate.
    pub fn position_ms(&self, now_ms: u64) -> u64 {
        if self.paused || self.phase != Phase::Open {
            return self.base_ms;
        }
        let rate = ppm_to_speed(self.rate_ppm);
        let latest = self.steps.back().map_or(self.base_at_ms, |&(at, _)| at);
        // Past the cap VLC is stuck (or a step is late): the estimate stops.
        let until = now_ms.min(latest + MAX_EXTRAPOLATE_MS);
        let from = |at: u64, ms: u64| ms + (until.saturating_sub(at) as f64 * rate).round() as u64;
        match self.steps.iter().map(|&(at, ms)| from(at, ms)).max() {
            Some(best) => best,
            None => from(self.base_at_ms, self.base_ms),
        }
    }

    pub fn observe(&mut self, now_ms: u64, status: Option<&Status>) -> Vec<Action> {
        match status {
            None => self.input_gone(now_ms),
            Some(s) => match s.state {
                InputState::Ended => self.input_gone(now_ms),
                InputState::Error => match self.phase {
                    Phase::Loading { .. } => self.fail(),
                    _ => self.input_gone(now_ms),
                },
                InputState::Opening => Vec::new(),
                InputState::Playing | InputState::Paused => self.input_open(now_ms, s),
            },
        }
    }

    fn fail(&mut self) -> Vec<Action> {
        self.phase = Phase::Failed;
        vec![Action::Event(PlayerEvent::LoadFailed {
            message: "VLC could not open the media".into(),
        })]
    }

    fn input_gone(&mut self, now_ms: u64) -> Vec<Action> {
        match self.phase {
            Phase::Idle | Phase::Ended | Phase::Failed => Vec::new(),
            Phase::Loading { since_ms, .. } => {
                if now_ms.saturating_sub(since_ms) >= LOAD_FAIL_MS {
                    self.fail()
                } else {
                    Vec::new()
                }
            }
            Phase::Open => {
                let position_ms = self.position_ms(now_ms);
                let near_end = self.length_ms.is_none_or(|len| position_ms + END_WINDOW_MS >= len);
                if near_end {
                    self.base_ms = position_ms;
                    self.phase = Phase::Ended;
                    vec![Action::Event(PlayerEvent::Eof)]
                } else {
                    self.phase = Phase::Loading {
                        since_ms: now_ms,
                        restore: Some(Restore {
                            position_ms,
                            paused: self.paused,
                            report_seek: false,
                        }),
                    };
                    vec![Action::Reload]
                }
            }
        }
    }

    fn speed(&self) -> f64 {
        let reported = ppm_to_speed(self.rate_ppm);
        match self.requested_speed {
            Some(r) if (r - reported).abs() < RATE_MATCH => r,
            _ => reported,
        }
    }

    fn rebase(&mut self, position_ms: u64, now_ms: u64) {
        self.base_ms = position_ms;
        self.base_at_ms = now_ms;
        self.steps.clear();
    }

    fn step(&mut self, time_ms: u64, now_ms: u64) {
        if self.steps.len() == STEP_WINDOW {
            self.steps.pop_front();
        }
        self.steps.push_back((now_ms, time_ms));
    }

    fn input_open(&mut self, now_ms: u64, s: &Status) -> Vec<Action> {
        let time_ms = s.time_us / 1000;
        let length_ms = (s.length_us > 0).then_some(s.length_us / 1000);
        let mut out = Vec::new();
        match self.phase {
            Phase::Idle | Phase::Ended | Phase::Failed => return out,
            Phase::Loading { restore, .. } => {
                self.phase = Phase::Open;
                self.paused = s.state == InputState::Paused;
                self.rate_ppm = s.rate_ppm;
                self.raw_ms = time_ms;
                self.rebase(time_ms, now_ms);
                self.length_ms = length_ms;
                match restore {
                    None => {
                        let title = (!s.title.is_empty()).then(|| s.title.clone());
                        out.push(Action::Event(PlayerEvent::FileLoaded {
                            duration_ms: length_ms,
                            title,
                        }));
                        out.push(Action::AddSubtitles);
                    }
                    Some(r) => {
                        out.push(Action::AddSubtitles);
                        out.push(Action::Restore { position_ms: r.position_ms, paused: r.paused });
                        // VLC opens paused; the restore must not look like the user's doing.
                        self.paused = r.paused;
                        self.expected_pause = Some((r.paused, now_ms));
                        self.rebase(r.position_ms, now_ms);
                        self.pending_seek = Some(PendingSeek {
                            target_ms: r.position_ms,
                            sent_ms: now_ms,
                            internal: !r.report_seek,
                        });
                    }
                }
                return out;
            }
            Phase::Open => {}
        }
        if length_ms.is_some() {
            self.length_ms = length_ms;
        }

        if s.rate_ppm != self.rate_ppm {
            let position = self.position_ms(now_ms);
            self.rate_ppm = s.rate_ppm;
            self.rebase(position, now_ms);
            out.push(Action::Event(PlayerEvent::Speed(self.speed())));
        }

        let paused = s.state == InputState::Paused;
        if let Some((expected, since)) = self.expected_pause
            && (paused == expected || now_ms.saturating_sub(since) > SEEK_FORGET_MS)
        {
            self.expected_pause = None;
        }
        if paused != self.paused && self.expected_pause.is_none() {
            let position = self.position_ms(now_ms);
            self.paused = paused;
            self.rebase(position, now_ms);
            out.push(Action::Event(PlayerEvent::Pause(paused)));
        }

        if let Some(seek) = self.pending_seek {
            if time_ms.abs_diff(seek.target_ms) <= SEEK_MATCH_MS {
                self.pending_seek = None;
                self.raw_ms = time_ms;
                self.rebase(time_ms, now_ms);
                if !seek.internal {
                    out.push(Action::Event(PlayerEvent::Seeked { position_ms: time_ms }));
                }
            } else if now_ms.saturating_sub(seek.sent_ms) > SEEK_FORGET_MS {
                self.pending_seek = None;
            }
        }
        if self.pending_seek.is_none() && time_ms != self.raw_ms {
            let jump = if self.paused {
                time_ms.abs_diff(self.raw_ms) > PAUSED_JUMP_MS
            } else {
                time_ms.abs_diff(self.position_ms(now_ms)) > USER_SEEK_MS
            };
            self.raw_ms = time_ms;
            if jump {
                self.rebase(time_ms, now_ms);
            } else if !self.paused {
                self.step(time_ms, now_ms);
            }
            if jump {
                out.push(Action::Event(PlayerEvent::Seeked { position_ms: time_ms }));
            }
        }

        if s.cache_permille < 1000 {
            let since = *self.cache_low_since.get_or_insert(now_ms);
            if !self.buffering && now_ms.saturating_sub(since) >= BUFFERING_AFTER_MS {
                self.buffering = true;
                out.push(Action::Event(PlayerEvent::Buffering(true)));
            }
        } else {
            self.cache_low_since = None;
            if self.buffering {
                self.buffering = false;
                out.push(Action::Event(PlayerEvent::Buffering(false)));
            }
        }

        let position = self.position_ms(now_ms);
        if !self.paused || self.last_reported != Some(position) {
            self.last_reported = Some(position);
            out.push(Action::Event(PlayerEvent::TimePos(position)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEN: u64 = 600_000;

    fn st(time_ms: u64, state: InputState) -> Status {
        Status {
            time_us: time_ms * 1000,
            length_us: LEN * 1000,
            state,
            rate_ppm: 1_000_000,
            cache_permille: 1000,
            title: "Episode 1".into(),
        }
    }

    fn playing(time_ms: u64) -> Status {
        st(time_ms, InputState::Playing)
    }

    fn paused(time_ms: u64) -> Status {
        st(time_ms, InputState::Paused)
    }

    fn events(actions: Vec<Action>) -> Vec<PlayerEvent> {
        actions
            .into_iter()
            .filter_map(|a| match a {
                Action::Event(e) => Some(e),
                _ => None,
            })
            .collect()
    }

    /// An item opened paused at 0 at time 1000, then playing from 2000.
    fn open_and_play() -> Tracker {
        let mut t = Tracker::new();
        t.load_sent(1000);
        t.observe(1000, Some(&paused(0)));
        t.observe(2000, Some(&playing(0)));
        t
    }

    #[test]
    fn opening_reports_the_file_and_asks_for_subtitles_but_no_pause() {
        let mut t = Tracker::new();
        t.load_sent(0);
        assert_eq!(t.observe(10, None), vec![]);
        assert_eq!(t.observe(40, Some(&st(0, InputState::Opening))), vec![]);
        assert_eq!(
            t.observe(70, Some(&paused(0))),
            vec![
                Action::Event(PlayerEvent::FileLoaded {
                    duration_ms: Some(LEN),
                    title: Some("Episode 1".into())
                }),
                Action::AddSubtitles,
            ]
        );
    }

    #[test]
    fn a_stream_without_length_has_no_duration() {
        let mut t = Tracker::new();
        t.load_sent(0);
        let s = Status { length_us: 0, title: String::new(), ..paused(0) };
        assert_eq!(
            events(t.observe(10, Some(&s))),
            vec![PlayerEvent::FileLoaded { duration_ms: None, title: None }]
        );
    }

    #[test]
    fn time_is_extrapolated_between_steps_and_capped() {
        let mut t = open_and_play();
        assert_eq!(events(t.observe(2100, Some(&playing(0)))), vec![PlayerEvent::TimePos(100)]);
        // The step VLC reports at 2250 anchors the estimate.
        assert_eq!(events(t.observe(2260, Some(&playing(250)))), vec![PlayerEvent::TimePos(250)]);
        assert_eq!(events(t.observe(2400, Some(&playing(250)))), vec![PlayerEvent::TimePos(390)]);
        // No step for long: VLC is stuck, the estimate stops.
        assert_eq!(
            events(t.observe(5000, Some(&playing(250)))),
            vec![PlayerEvent::TimePos(250 + MAX_EXTRAPOLATE_MS)]
        );
    }

    #[test]
    fn late_noticed_steps_do_not_pull_the_estimate_back() {
        let mut t = open_and_play();
        // VLC steps every 250 ms from 2000 (exactly on time); polling notices them
        // 0..20 ms late.
        for (i, late) in [(1, 0), (2, 15), (3, 20), (4, 5), (5, 18)] {
            let step = 2000 + i * 250;
            t.observe(step + late, Some(&playing(i * 250)));
        }
        // The step at 3250 was noticed 18 ms late; the one at 2250 on time.
        assert_eq!(events(t.observe(3300, Some(&playing(1250)))), vec![PlayerEvent::TimePos(1300)]);
    }

    #[test]
    fn a_step_a_little_off_the_estimate_is_no_seek() {
        let mut t = open_and_play();
        t.observe(2500, Some(&playing(0)));
        assert_eq!(events(t.observe(2510, Some(&playing(400)))), vec![PlayerEvent::TimePos(400)]);
    }

    #[test]
    fn our_seek_is_reported_once_at_the_target() {
        let mut t = open_and_play();
        assert!(!t.seek_sent(30_000, 2100));
        assert_eq!(
            events(t.observe(2110, Some(&playing(30_000)))),
            vec![PlayerEvent::Seeked { position_ms: 30_000 }, PlayerEvent::TimePos(30_000)]
        );
        assert_eq!(
            events(t.observe(2400, Some(&playing(30_250)))),
            vec![PlayerEvent::TimePos(30_250)]
        );
    }

    #[test]
    fn a_jump_made_in_vlc_is_a_seek() {
        let mut t = open_and_play();
        assert_eq!(
            events(t.observe(2300, Some(&playing(90_000)))),
            vec![PlayerEvent::Seeked { position_ms: 90_000 }, PlayerEvent::TimePos(90_000)]
        );
    }

    #[test]
    fn a_pause_keeps_the_estimated_position_not_the_stale_step() {
        let mut t = open_and_play();
        t.observe(2250, Some(&playing(250)));
        // Paused at about 450 while VLC still says 250.
        assert_eq!(
            events(t.observe(2450, Some(&paused(250)))),
            vec![PlayerEvent::Pause(true), PlayerEvent::TimePos(450)]
        );
        assert_eq!(events(t.observe(2600, Some(&paused(250)))), vec![]);
        assert_eq!(
            events(t.observe(3000, Some(&playing(250)))),
            vec![PlayerEvent::Pause(false), PlayerEvent::TimePos(450)]
        );
        assert_eq!(events(t.observe(3100, Some(&playing(250)))), vec![PlayerEvent::TimePos(550)]);
    }

    #[test]
    fn a_jump_while_paused_is_a_seek() {
        let mut t = open_and_play();
        t.observe(2100, Some(&paused(0)));
        assert_eq!(
            events(t.observe(2200, Some(&paused(10_000)))),
            vec![PlayerEvent::Seeked { position_ms: 10_000 }, PlayerEvent::TimePos(10_000)]
        );
    }

    #[test]
    fn the_speed_we_asked_for_is_reported_despite_vlc_rounding() {
        let mut t = open_and_play();
        t.speed_sent(1.05);
        let s = Status { rate_ppm: 1_050_420, ..playing(0) };
        assert_eq!(
            events(t.observe(2100, Some(&s))),
            vec![PlayerEvent::Speed(1.05), PlayerEvent::TimePos(100)]
        );
        // A speed the user picks in VLC is reported as VLC has it.
        let s = Status { rate_ppm: 1_500_000, ..playing(0) };
        assert_eq!(
            events(t.observe(2200, Some(&s))),
            vec![PlayerEvent::Speed(1.5), PlayerEvent::TimePos(205)]
        );
    }

    #[test]
    fn nothing_opened_for_a_while_is_a_load_failure() {
        let mut t = Tracker::new();
        t.load_sent(0);
        assert_eq!(t.observe(LOAD_FAIL_MS - 1, None), vec![]);
        assert_eq!(
            events(t.observe(LOAD_FAIL_MS, None)),
            vec![PlayerEvent::LoadFailed { message: "VLC could not open the media".into() }]
        );
        assert_eq!(t.observe(LOAD_FAIL_MS + 100, None), vec![]);
    }

    #[test]
    fn an_error_state_while_loading_fails_at_once() {
        let mut t = Tracker::new();
        t.load_sent(0);
        assert!(matches!(
            events(t.observe(50, Some(&st(0, InputState::Error))))[..],
            [PlayerEvent::LoadFailed { .. }]
        ));
    }

    #[test]
    fn the_input_going_away_at_the_end_is_eof() {
        let mut t = open_and_play();
        t.seek_sent(LEN - 1000, 2100);
        t.observe(2110, Some(&playing(LEN - 1000)));
        assert_eq!(
            events(t.observe(3000, Some(&st(LEN, InputState::Ended)))),
            vec![PlayerEvent::Eof]
        );
        assert_eq!(t.observe(3100, None), vec![]);
    }

    #[test]
    fn stop_in_the_middle_reopens_the_item_where_it_was() {
        let mut t = open_and_play();
        t.observe(2250, Some(&playing(250)));
        assert_eq!(t.observe(2300, None), vec![Action::Reload]);
        assert_eq!(t.observe(2400, None), vec![]);
        // VLC opens it paused at 0; the restore puts it back without any events.
        assert_eq!(
            t.observe(2500, Some(&paused(0))),
            vec![Action::AddSubtitles, Action::Restore { position_ms: 300, paused: false }]
        );
        assert_eq!(events(t.observe(2510, Some(&paused(300)))), vec![PlayerEvent::TimePos(300)]);
        assert_eq!(events(t.observe(2600, Some(&playing(300)))), vec![PlayerEvent::TimePos(390)]);
    }

    #[test]
    fn a_seek_after_the_end_reopens_the_item_and_reports_the_seek() {
        let mut t = open_and_play();
        t.seek_sent(LEN - 500, 2100);
        t.observe(2110, Some(&playing(LEN - 500)));
        t.observe(2700, None);
        assert!(t.seek_sent(60_000, 3000));
        assert_eq!(
            t.observe(3200, Some(&paused(0))),
            vec![Action::AddSubtitles, Action::Restore { position_ms: 60_000, paused: false }]
        );
        assert_eq!(
            events(t.observe(3250, Some(&playing(60_000)))),
            vec![PlayerEvent::Seeked { position_ms: 60_000 }, PlayerEvent::TimePos(60_000)]
        );
    }

    #[test]
    fn short_cache_dips_are_not_buffering() {
        let mut t = open_and_play();
        let low = Status { cache_permille: 0, ..playing(0) };
        assert_eq!(events(t.observe(2100, Some(&low))), vec![PlayerEvent::TimePos(100)]);
        assert_eq!(events(t.observe(2200, Some(&playing(0)))), vec![PlayerEvent::TimePos(200)]);
        t.observe(3000, Some(&low));
        assert_eq!(
            events(t.observe(3000 + BUFFERING_AFTER_MS, Some(&low))),
            vec![PlayerEvent::Buffering(true), PlayerEvent::TimePos(750)]
        );
        assert_eq!(
            events(t.observe(3600, Some(&playing(1000)))),
            vec![PlayerEvent::Buffering(false), PlayerEvent::TimePos(1000)]
        );
    }
}
