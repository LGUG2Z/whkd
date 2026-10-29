//! Working out what the fingers on a touchpad are doing.
//!
//! Three or four fingers that land together start a gesture. From then on the recogniser says how
//! they move, which is what a stream binding is told, and when they have moved far enough in one
//! direction it says so once, which is what a swipe binding runs on.
//!
//! The thresholds are estimates, and need tuning on a real touchpad: they are in one place, in
//! [`Thresholds`].

use super::frame::Frame;
use std::collections::HashMap;
use std::time::Duration;
use whkd_core::GestureInput;
use whkd_core::SwipeDirection;

/// What a recogniser says
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Output {
    /// Fingers landed together and stayed: a gesture has begun. `fingers` is 3 or 4, and `x_cm`
    /// and `y_cm` are the centroid of the fingers, in centimetres from the touchpad's top left
    Begin {
        fingers: u8,
        x_cm: f64,
        y_cm: f64,
        time: Duration,
    },
    /// The fingers moved this far since the last `Move`, in centimetres: `dx_cm` is positive to the
    /// right and `dy_cm` downwards. `x_cm` and `y_cm` are where their centroid is now. `time` is
    /// when, from the frame, so that whoever receives these can work out how fast
    Move {
        dx_cm: f64,
        dy_cm: f64,
        x_cm: f64,
        y_cm: f64,
        time: Duration,
    },
    /// The fingers lifted, with their centroid last where `x_cm` and `y_cm` say
    End {
        x_cm: f64,
        y_cm: f64,
        time: Duration,
    },
    /// The gesture was abandoned (the number of fingers changed, or the gesture was stopped from
    /// outside), and there is no `End` for it
    Cancel,
    /// The fingers have moved far enough in one direction to be a swipe. Said once per gesture
    Swipe(GestureInput),
}

/// How the recogniser tells a gesture from other things the fingers do
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Thresholds {
    /// Fingers of one gesture must all have landed within this long of the first one touching, so
    /// that a third finger joining a two finger scroll is not a gesture
    pub landing_window: Duration,
    /// The fingers must stay as many as they are for this long before the gesture begins, so that
    /// four fingers landing a few milliseconds apart are not first three
    pub settle: Duration,
    /// How far three fingers move for a swipe, in centimetres
    pub swipe3_cm: f64,
    /// How far four fingers move for a swipe, in centimetres
    pub swipe4_cm: f64,
    /// A swipe goes along the axis it moves most on, and only if that is this many times more than
    /// the other, so a diagonal is not a swipe
    pub dominance: f64,
    /// After a swipe fires, another one does not for this long
    pub cooldown: Duration,
    /// Movement smaller than this is noise, and not reported
    pub noise_cm: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            landing_window: Duration::from_millis(100),
            settle: Duration::from_millis(40),
            swipe3_cm: 1.5,
            swipe4_cm: 1.2,
            dominance: 1.5,
            cooldown: Duration::from_millis(300),
            noise_cm: 0.005,
        }
    }
}

#[derive(Debug)]
struct Active {
    fingers: u8,
    /// Where each finger was in the last frame
    last: HashMap<u32, (f64, f64)>,
    /// Where their centroid was in the last frame
    centroid: (f64, f64),
    /// How far the fingers have moved since the gesture began
    total: (f64, f64),
    /// Whether the swipe was already said, or passed up
    swiped: bool,
}

#[derive(Debug)]
enum State {
    /// Nothing is touching
    Idle,
    /// One or two fingers are touching (since `started`), perhaps with more to come
    Landing {
        started: Duration,
    },
    /// As many fingers as a gesture needs have landed, and are being given time to settle
    Settling {
        started: Duration,
        fingers: u8,
        since: Duration,
    },
    Active(Active),
    /// This touch is not a gesture, and is left alone until every finger has lifted
    Ignoring,
}

#[derive(Debug)]
pub struct Recogniser {
    thresholds: Thresholds,
    state: State,
    last_swipe: Option<Duration>,
}

impl Recogniser {
    #[must_use]
    pub fn new(thresholds: Thresholds) -> Self {
        Self {
            thresholds,
            state: State::Idle,
            last_swipe: None,
        }
    }

    /// Abandon a gesture that is under way, because something outside it says so (whkd is being
    /// paused, or the touchpad went away). `Some(Cancel)` if there was one
    pub fn reset(&mut self) -> Option<Output> {
        let was_active = matches!(self.state, State::Active(_));
        // nothing is touching: there is nothing to leave alone until it lifts
        if !matches!(self.state, State::Idle) {
            self.state = State::Ignoring;
        }
        was_active.then_some(Output::Cancel)
    }

    /// What the fingers did in `frame`
    pub fn frame(&mut self, frame: &Frame) -> Vec<Output> {
        let fingers: Vec<(u32, f64, f64)> = frame
            .contacts
            .iter()
            .filter(|contact| contact.confident)
            .map(|contact| (contact.id, contact.x_cm, contact.y_cm))
            .collect();

        let mut out = vec![];
        let now = frame.time;
        let count = fingers.len();

        loop {
            match &mut self.state {
                State::Idle => {
                    if count == 0 {
                        break;
                    }
                    self.state = State::Landing { started: now };
                }

                State::Landing { started } => {
                    let started = *started;
                    match count {
                        0 => self.state = State::Idle,
                        3 | 4 => {
                            self.state =
                                if now.saturating_sub(started) <= self.thresholds.landing_window {
                                    State::Settling {
                                        started,
                                        fingers: count as u8,
                                        since: now,
                                    }
                                } else {
                                    State::Ignoring
                                };
                        }
                        1 | 2 => {}
                        _ => self.state = State::Ignoring,
                    }
                    break;
                }

                State::Settling {
                    started,
                    fingers: expected,
                    since,
                } => {
                    let (started, expected, since) = (*started, *expected, *since);
                    match count {
                        0 => self.state = State::Idle,
                        3 | 4 if count == usize::from(expected) => {
                            if now.saturating_sub(since) >= self.thresholds.settle {
                                let centroid = centroid(&fingers);
                                out.push(Output::Begin {
                                    fingers: expected,
                                    x_cm: centroid.0,
                                    y_cm: centroid.1,
                                    time: now,
                                });
                                self.state = State::Active(Active {
                                    fingers: expected,
                                    last: fingers.iter().map(|&(id, x, y)| (id, (x, y))).collect(),
                                    centroid,
                                    total: (0.0, 0.0),
                                    swiped: false,
                                });
                            }
                        }
                        // another finger landed, or one more of the same still to come
                        3 | 4 => {
                            self.state =
                                if now.saturating_sub(started) <= self.thresholds.landing_window {
                                    State::Settling {
                                        started,
                                        fingers: count as u8,
                                        since: now,
                                    }
                                } else {
                                    State::Ignoring
                                };
                        }
                        _ => self.state = State::Ignoring,
                    }
                    break;
                }

                State::Active(active) => {
                    if count == 0 {
                        // the last finger lifted
                        out.push(Output::End {
                            x_cm: active.centroid.0,
                            y_cm: active.centroid.1,
                            time: now,
                        });
                        self.state = State::Idle;
                        break;
                    }

                    if count != usize::from(active.fingers) {
                        if count < usize::from(active.fingers) {
                            // fingers lift one at a time: that is the end of the gesture, and the
                            // rest of the lift is not another
                            out.push(Output::End {
                                x_cm: active.centroid.0,
                                y_cm: active.centroid.1,
                                time: now,
                            });
                            self.state = State::Ignoring;
                        } else if count == 4 && active.fingers == 3 {
                            // a fourth finger landed: the three finger gesture was not one, and
                            // this may be a four finger one
                            out.push(Output::Cancel);
                            self.state = State::Settling {
                                started: now,
                                fingers: 4,
                                since: now,
                            };
                        } else {
                            out.push(Output::Cancel);
                            self.state = State::Ignoring;
                        }
                        break;
                    }

                    follow(
                        active,
                        &fingers,
                        now,
                        &self.thresholds,
                        &mut self.last_swipe,
                        &mut out,
                    );
                    break;
                }

                State::Ignoring => {
                    if count == 0 {
                        self.state = State::Idle;
                    }
                    break;
                }
            }
        }

        out
    }
}

/// How the fingers of an active gesture moved in this frame
fn follow(
    active: &mut Active,
    fingers: &[(u32, f64, f64)],
    now: Duration,
    thresholds: &Thresholds,
    last_swipe: &mut Option<Duration>,
    out: &mut Vec<Output>,
) {
    // how far the fingers that were there before have moved, on average
    let moved: Vec<(f64, f64)> = fingers
        .iter()
        .filter_map(|&(id, x, y)| {
            active
                .last
                .get(&id)
                .map(|&(last_x, last_y)| (x - last_x, y - last_y))
        })
        .collect();

    active.last = fingers.iter().map(|&(id, x, y)| (id, (x, y))).collect();

    if moved.is_empty() {
        return;
    }

    let n = moved.len() as f64;
    let dx = moved.iter().map(|(dx, _)| dx).sum::<f64>() / n;
    let dy = moved.iter().map(|(_, dy)| dy).sum::<f64>() / n;

    active.total.0 += dx;
    active.total.1 += dy;
    active.centroid = centroid(fingers);

    if dx.abs() >= thresholds.noise_cm || dy.abs() >= thresholds.noise_cm {
        out.push(Output::Move {
            dx_cm: dx,
            dy_cm: dy,
            x_cm: active.centroid.0,
            y_cm: active.centroid.1,
            time: now,
        });
    }

    if !active.swiped {
        if let Some(direction) = swipe(active.total, active.fingers, thresholds) {
            active.swiped = true;

            let cooled_down =
                last_swipe.is_none_or(|at| now.saturating_sub(at) >= thresholds.cooldown);
            if cooled_down {
                *last_swipe = Some(now);
                if let Some(gesture) = GestureInput::swipe(active.fingers, direction) {
                    out.push(Output::Swipe(gesture));
                }
            }
        }
    }
}

/// The middle of the fingers
fn centroid(fingers: &[(u32, f64, f64)]) -> (f64, f64) {
    let n = fingers.len().max(1) as f64;
    (
        fingers.iter().map(|&(_, x, _)| x).sum::<f64>() / n,
        fingers.iter().map(|&(_, _, y)| y).sum::<f64>() / n,
    )
}

/// The direction the fingers have gone, once they have gone far enough along one axis
fn swipe(total: (f64, f64), fingers: u8, thresholds: &Thresholds) -> Option<SwipeDirection> {
    let needed = if fingers == 3 {
        thresholds.swipe3_cm
    } else {
        thresholds.swipe4_cm
    };

    let (dx, dy) = total;
    let (major, minor) = if dx.abs() >= dy.abs() {
        (dx.abs(), dy.abs())
    } else {
        (dy.abs(), dx.abs())
    };

    if major < needed || major < minor * thresholds.dominance {
        return None;
    }

    Some(if dx.abs() >= dy.abs() {
        if dx > 0.0 {
            SwipeDirection::Right
        } else {
            SwipeDirection::Left
        }
    } else if dy > 0.0 {
        SwipeDirection::Down
    } else {
        SwipeDirection::Up
    })
}

#[cfg(test)]
mod tests {
    use super::super::frame::Contact;
    use super::*;

    const FRAME_MS: u64 = 8;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Fingers going along together from `from` to `to`, a frame at a time
    struct Script {
        recogniser: Recogniser,
        time: u64,
        out: Vec<Output>,
    }

    impl Script {
        fn new() -> Self {
            Self::with(Thresholds::default())
        }

        fn with(thresholds: Thresholds) -> Self {
            Self {
                recogniser: Recogniser::new(thresholds),
                time: 0,
                out: vec![],
            }
        }

        fn frame(&mut self, fingers: &[(u32, f64, f64)]) {
            let frame = Frame::new(ms(self.time), fingers);
            self.out.extend(self.recogniser.frame(&frame));
            self.time += FRAME_MS;
        }

        fn lift(&mut self) {
            self.frame(&[]);
        }

        /// `n` fingers side by side, which land at once
        fn land(&mut self, n: u32, x: f64, y: f64) {
            self.hold(n, x, y, 10);
        }

        fn hold(&mut self, n: u32, x: f64, y: f64, frames: u32) {
            for _ in 0..frames {
                self.frame(&fingers(n, x, y));
            }
        }

        /// Move `n` fingers by (dx, dy) in `frames` frames, from where they are
        fn drag(&mut self, n: u32, from: (f64, f64), by: (f64, f64), frames: u32) {
            for step in 1..=frames {
                let t = f64::from(step) / f64::from(frames);
                self.frame(&fingers(n, from.0 + by.0 * t, from.1 + by.1 * t));
            }
        }

        fn swipes(&self) -> Vec<GestureInput> {
            self.out
                .iter()
                .filter_map(|output| match output {
                    Output::Swipe(gesture) => Some(*gesture),
                    _ => None,
                })
                .collect()
        }

        fn begins(&self) -> Vec<u8> {
            self.out
                .iter()
                .filter_map(|output| match output {
                    Output::Begin { fingers, .. } => Some(*fingers),
                    _ => None,
                })
                .collect()
        }

        /// How far the fingers moved, in total, as streamed
        fn moved(&self) -> (f64, f64) {
            self.out
                .iter()
                .fold((0.0, 0.0), |(x, y), output| match output {
                    Output::Move { dx_cm, dy_cm, .. } => (x + dx_cm, y + dy_cm),
                    _ => (x, y),
                })
        }

        fn moves(&self) -> Vec<(f64, f64, Duration)> {
            self.out
                .iter()
                .filter_map(|output| match output {
                    Output::Move {
                        dx_cm, dy_cm, time, ..
                    } => Some((*dx_cm, *dy_cm, *time)),
                    _ => None,
                })
                .collect()
        }

        fn ends(&self) -> usize {
            self.out
                .iter()
                .filter(|output| matches!(output, Output::End { .. }))
                .count()
        }

        fn count(&self, wanted: Output) -> usize {
            self.out.iter().filter(|output| **output == wanted).count()
        }
    }

    /// `n` fingers a centimetre apart
    fn fingers(n: u32, x: f64, y: f64) -> Vec<(u32, f64, f64)> {
        (0..n).map(|i| (i, x + f64::from(i), y)).collect()
    }

    #[test]
    fn three_fingers_swiping_right() {
        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (2.4, 0.0), 20);
        s.lift();

        assert_eq!(s.begins(), vec![3]);
        assert_eq!(s.swipes(), vec![GestureInput::Swipe3Right]);
        assert!((s.moved().0 - 2.4).abs() < 1e-9, "{:?}", s.moved());
        assert!(s.moved().1.abs() < 1e-9);
        assert_eq!(s.ends(), 1);
    }

    #[test]
    fn the_four_directions() {
        for (by, expected) in [
            ((-2.0, 0.0), GestureInput::Swipe3Left),
            ((2.0, 0.0), GestureInput::Swipe3Right),
            ((0.0, -2.0), GestureInput::Swipe3Up),
            ((0.0, 2.0), GestureInput::Swipe3Down),
        ] {
            let mut s = Script::new();
            s.land(3, 4.0, 4.0);
            s.drag(3, (4.0, 4.0), by, 20);
            s.lift();
            assert_eq!(s.swipes(), vec![expected], "{by:?}");
        }
    }

    #[test]
    fn four_fingers_have_their_own_gestures_and_a_shorter_distance() {
        let mut s = Script::new();
        s.land(4, 3.0, 3.0);
        // 1.3 cm is enough for four fingers, and not for three
        s.drag(4, (3.0, 3.0), (-1.3, 0.0), 12);
        s.lift();

        assert_eq!(s.begins(), vec![4]);
        assert_eq!(s.swipes(), vec![GestureInput::Swipe4Left]);

        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (-1.3, 0.0), 12);
        s.lift();
        assert!(s.swipes().is_empty());
    }

    #[test]
    fn a_short_movement_is_not_a_swipe_but_is_still_streamed() {
        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (0.6, 0.0), 10);
        s.lift();

        assert!(s.swipes().is_empty());
        assert_eq!(s.begins(), vec![3]);
        assert!((s.moved().0 - 0.6).abs() < 1e-9);
        assert_eq!(s.ends(), 1);
    }

    #[test]
    fn a_diagonal_is_not_a_swipe() {
        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (1.8, 1.7), 20);
        s.lift();
        assert!(s.swipes().is_empty());

        // but going mostly one way, with some drift, is
        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (2.0, 0.6), 20);
        s.lift();
        assert_eq!(s.swipes(), vec![GestureInput::Swipe3Right]);
    }

    #[test]
    fn a_swipe_is_said_once_however_far_it_goes() {
        let mut s = Script::new();
        s.land(3, 1.0, 3.0);
        s.drag(3, (1.0, 3.0), (6.0, 0.0), 40);
        s.lift();
        assert_eq!(s.swipes(), vec![GestureInput::Swipe3Right]);
    }

    #[test]
    fn stationary_fingers_are_not_movement() {
        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.hold(3, 3.0, 3.0, 30);
        s.lift();

        assert_eq!(s.begins(), vec![3]);
        assert_eq!(s.moved(), (0.0, 0.0));
        assert!(s.moves().is_empty());
        assert_eq!(s.ends(), 1);
    }

    #[test]
    fn the_gesture_begins_after_the_fingers_settle_and_not_before() {
        let mut s = Script::new();
        // 4 frames of 8 ms is not 40 ms yet
        s.hold(3, 3.0, 3.0, 4);
        assert!(s.begins().is_empty());
        s.hold(3, 3.0, 3.0, 3);
        assert_eq!(s.begins(), vec![3]);
    }

    #[test]
    fn fingers_that_lift_before_they_settle_are_nothing() {
        let mut s = Script::new();
        s.hold(3, 3.0, 3.0, 3);
        s.lift();
        assert!(s.out.is_empty(), "{:?}", s.out);
    }

    #[test]
    fn four_fingers_landing_a_few_milliseconds_apart_are_not_first_three() {
        let mut s = Script::new();
        s.hold(2, 3.0, 3.0, 2);
        s.hold(3, 3.0, 3.0, 2);
        s.hold(4, 3.0, 3.0, 10);
        s.drag(4, (3.0, 3.0), (-2.0, 0.0), 15);
        s.lift();

        assert_eq!(s.begins(), vec![4]);
        assert_eq!(s.swipes(), vec![GestureInput::Swipe4Left]);
        assert_eq!(s.count(Output::Cancel), 0);
    }

    #[test]
    fn a_third_finger_joining_a_two_finger_scroll_is_not_a_gesture() {
        let mut s = Script::new();
        // two fingers scrolling for half a second
        s.hold(2, 3.0, 3.0, 60);
        s.hold(3, 3.0, 3.0, 20);
        s.drag(3, (3.0, 3.0), (2.5, 0.0), 20);
        s.lift();

        assert!(s.out.is_empty(), "{:?}", s.out);
    }

    #[test]
    fn a_gesture_is_not_started_by_five_fingers_or_more() {
        let mut s = Script::new();
        s.land(5, 1.0, 3.0);
        s.drag(5, (1.0, 3.0), (3.0, 0.0), 20);
        s.lift();
        assert!(s.out.is_empty(), "{:?}", s.out);
    }

    #[test]
    fn a_finger_lifting_ends_the_gesture_once() {
        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (0.8, 0.0), 10);
        // the fingers lift one at a time
        s.frame(&fingers(2, 3.8, 3.0));
        s.frame(&fingers(1, 3.8, 3.0));
        s.lift();

        assert_eq!(s.ends(), 1);
        assert_eq!(s.begins(), vec![3]);
        assert_eq!(s.count(Output::Cancel), 0);
    }

    #[test]
    fn what_is_left_of_a_lift_is_not_another_gesture() {
        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.frame(&fingers(2, 3.0, 3.0));
        // two fingers resting, then a third landing again, all one touch
        s.hold(2, 3.0, 3.0, 20);
        s.hold(3, 3.0, 3.0, 20);
        s.lift();

        assert_eq!(s.begins(), vec![3]);
        assert_eq!(s.ends(), 1);
    }

    #[test]
    fn a_fourth_finger_makes_it_a_four_finger_gesture() {
        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (0.3, 0.0), 5);
        s.hold(4, 3.3, 3.0, 10);
        s.drag(4, (3.3, 3.0), (-2.0, 0.0), 15);
        s.lift();

        assert_eq!(s.begins(), vec![3, 4]);
        assert_eq!(s.count(Output::Cancel), 1);
        assert_eq!(s.swipes(), vec![GestureInput::Swipe4Left]);
        assert_eq!(s.ends(), 1);
    }

    #[test]
    fn a_palm_is_not_a_finger() {
        let palm = |id, x, y| Contact {
            id,
            x_cm: x,
            y_cm: y,
            confident: false,
        };
        let with_palm = |mut frame: Frame| {
            frame.contacts.push(palm(9, 8.0, 8.0));
            frame
        };

        // three fingers and a palm are three fingers
        let mut r = Recogniser::new(Thresholds::default());
        let mut out = vec![];
        for i in 0..10 {
            let frame = Frame::new(ms(i * FRAME_MS), &fingers(3, 3.0, 3.0));
            out.extend(r.frame(&with_palm(frame)));
        }
        assert!(
            matches!(out.as_slice(), [Output::Begin { fingers: 3, .. }]),
            "{out:?}"
        );

        // two fingers and a palm are not three
        let mut r = Recogniser::new(Thresholds::default());
        let mut out = vec![];
        for i in 0..20 {
            let frame = Frame::new(ms(i * FRAME_MS), &fingers(2, 3.0, 3.0));
            out.extend(r.frame(&with_palm(frame)));
        }
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_swipe_in_the_cooldown_is_passed_up_but_a_stream_is_not() {
        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (2.0, 0.0), 10);
        s.lift();
        // the next one starts 100 ms later, inside the 300 ms cooldown
        s.hold(0, 0.0, 0.0, 6);
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (2.0, 0.0), 10);
        s.lift();

        assert_eq!(s.swipes(), vec![GestureInput::Swipe3Right]);
        // but both were streamed in full
        assert_eq!(s.begins(), vec![3, 3]);
        assert_eq!(s.ends(), 2);

        // after the cooldown, it fires again
        s.hold(0, 0.0, 0.0, 60);
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (2.0, 0.0), 10);
        s.lift();
        assert_eq!(s.swipes().len(), 2);
    }

    #[test]
    fn a_swipe_that_missed_the_cooldown_does_not_fire_late() {
        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (2.0, 0.0), 6);
        s.lift();
        s.land(3, 3.0, 3.0);
        // far enough to be a swipe inside the cooldown, so it is passed up...
        s.drag(3, (3.0, 3.0), (2.0, 0.0), 12);
        // ...and carries on until the cooldown is over, without being a swipe then
        s.drag(3, (5.0, 3.0), (3.0, 0.0), 40);
        s.lift();
        assert_eq!(s.swipes(), vec![GestureInput::Swipe3Right]);
    }

    #[test]
    fn the_raw_data_says_where_the_fingers_are() {
        let mut s = Script::new();
        // three fingers a centimetre apart, the middle one at x = 4
        s.land(3, 3.0, 2.0);
        s.drag(3, (3.0, 2.0), (1.0, 0.5), 10);
        s.lift();

        match s.out.first() {
            Some(Output::Begin {
                fingers: 3,
                x_cm,
                y_cm,
                ..
            }) => {
                assert!((x_cm - 4.0).abs() < 1e-9 && (y_cm - 2.0).abs() < 1e-9);
            }
            other => panic!("{other:?}"),
        }

        let (x, y) = match s.out.iter().rev().nth(1) {
            Some(Output::Move { x_cm, y_cm, .. }) => (*x_cm, *y_cm),
            other => panic!("{other:?}"),
        };
        assert!((x - 5.0).abs() < 1e-9 && (y - 2.5).abs() < 1e-9, "{x} {y}");

        match s.out.last() {
            Some(Output::End { x_cm, y_cm, .. }) => {
                assert!((x_cm - 5.0).abs() < 1e-9 && (y_cm - 2.5).abs() < 1e-9);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn moves_carry_the_time_so_that_a_velocity_can_be_worked_out() {
        let mut s = Script::new();
        s.land(3, 6.0, 3.0);
        // a quick flick to the left: 2 cm in 6 frames of 8 ms
        s.drag(3, (6.0, 3.0), (-2.0, 0.0), 6);
        s.lift();

        let moves = s.moves();
        assert_eq!(moves.len(), 6);

        // the times are the frames', and go up by a frame at a time
        for pair in moves.windows(2) {
            assert_eq!(pair[1].2 - pair[0].2, ms(FRAME_MS));
        }

        // so dx over dt is the speed (from the first move to the last)
        let dx: f64 = moves[1..].iter().map(|m| m.0).sum();
        let dt = (moves[5].2 - moves[0].2).as_secs_f64();
        // 2 cm over 6 steps is a third of a centimetre a step, and 5 of them are 40 ms
        let expected = (-2.0 * 5.0 / 6.0) / 0.040;
        assert!((dx / dt - expected).abs() < 1.0, "{} {expected}", dx / dt);
    }

    #[test]
    fn vertical_movement_is_streamed_too() {
        let mut s = Script::new();
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (0.3, 1.2), 10);
        s.lift();

        let (dx, dy) = s.moved();
        assert!(
            (dx - 0.3).abs() < 1e-9 && (dy - 1.2).abs() < 1e-9,
            "{dx} {dy}"
        );
        // downwards is positive
        assert!(s.moves().iter().all(|(_, dy, _)| *dy >= 0.0));
    }

    #[test]
    fn moves_add_up_to_how_far_the_fingers_went() {
        let mut s = Script::new();
        s.land(3, 5.0, 3.0);
        s.drag(3, (5.0, 3.0), (-3.7, 0.0), 25);
        s.drag(3, (1.3, 3.0), (0.9, 0.0), 10);
        s.lift();
        assert!((s.moved().0 - (-2.8)).abs() < 1e-9, "{:?}", s.moved());
    }

    #[test]
    fn a_gesture_stopped_from_outside_is_cancelled() {
        let mut s = Script::new();
        assert_eq!(s.recogniser.reset(), None);

        s.land(3, 3.0, 3.0);
        assert_eq!(s.recogniser.reset(), Some(Output::Cancel));
        // the rest of the touch is not a gesture, and there is no End for it
        s.drag(3, (3.0, 3.0), (2.0, 0.0), 10);
        s.lift();
        assert_eq!(s.begins(), vec![3]);
        assert_eq!(s.ends(), 0);
        assert!(s.swipes().is_empty());

        // but the next touch is
        s.land(3, 3.0, 3.0);
        assert_eq!(s.begins(), vec![3, 3]);
    }

    #[test]
    fn a_touch_is_recognised_again_after_it_was_ignored() {
        let mut s = Script::new();
        s.land(5, 1.0, 3.0);
        s.lift();
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (2.0, 0.0), 12);
        s.lift();
        assert_eq!(s.begins(), vec![3]);
        assert_eq!(s.swipes(), vec![GestureInput::Swipe3Right]);
    }

    #[test]
    fn the_thresholds_can_be_changed() {
        let mut s = Script::with(Thresholds {
            swipe3_cm: 4.0,
            ..Thresholds::default()
        });
        s.land(3, 3.0, 3.0);
        s.drag(3, (3.0, 3.0), (3.0, 0.0), 20);
        s.lift();
        assert!(s.swipes().is_empty());
    }
}
