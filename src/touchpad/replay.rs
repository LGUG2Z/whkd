//! Frames to a file and back, and made-up gestures, so that everything after a touchpad's reports
//! can be tried without a touchpad: through the recogniser, as a stream, into a command.
//!
//! The text format is one frame to a line, so a capture can be read and edited by hand:
//!
//! ```text
//! # whkd touchpad frames v1
//! pad 11.7 7.5
//! 100000.0 1:4.000:3.000 2:5.000:3.000 3:6.000:3.000
//! 100008.0 1:4.100:3.000 2:5.100:3.000 3:6.100:3.000
//! 100016.0
//! ```
//!
//! A line is a time in milliseconds, then each finger as `id:x:y` in centimetres from the top left
//! of the touchpad (`id:x:y:palm` for one the touchpad does not believe is a finger). A line with
//! only a time is a frame with no fingers, which is how a lift looks. `pad <width> <height>`, in
//! centimetres, says how big the touchpad is, and `#` starts a comment.

use super::frame::Contact;
use super::frame::Frame;
use super::protocol::Pad;
use super::recogniser::Output;
use super::recogniser::Recogniser;
use super::recogniser::Thresholds;
use std::time::Duration;
use std::time::Instant;

/// The size of the touchpad the made-up gestures are made on
pub const DEMO_PAD: Pad = Pad {
    width_cm: 11.7,
    height_cm: 7.5,
};

/// A series of frames, and how big the touchpad was
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub pad: Option<Pad>,
    pub frames: Vec<Frame>,
}

fn millis(time: Duration) -> f64 {
    time.as_secs_f64() * 1000.0
}

/// A frame as a line of the format
#[must_use]
pub fn format_frame(frame: &Frame) -> String {
    let mut line = format!("{:.1}", millis(frame.time));
    for contact in &frame.contacts {
        line.push_str(&format!(
            " {}:{:.3}:{:.3}{}",
            contact.id,
            contact.x_cm,
            contact.y_cm,
            if contact.confident { "" } else { ":palm" }
        ));
    }
    line
}

/// The first lines of a recording
#[must_use]
pub fn header(pad: Option<Pad>) -> String {
    let mut text = String::from("# whkd touchpad frames v1\n");
    if let Some(pad) = pad {
        text.push_str(&format!("pad {:.1} {:.1}\n", pad.width_cm, pad.height_cm));
    }
    text
}

/// A recording as text
#[must_use]
pub fn format(recording: &Recording) -> String {
    let mut text = header(recording.pad);
    for frame in &recording.frames {
        text.push_str(&format_frame(frame));
        text.push('\n');
    }
    text
}

fn number(token: &str, what: &str, line: usize) -> Result<f64, String> {
    token
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("line {line}: {what} should be a number, not `{token}`"))
}

/// Read a recording
pub fn parse(text: &str) -> Result<Recording, String> {
    let mut pad = None;
    let mut frames: Vec<Frame> = vec![];

    for (index, line) in text.lines().enumerate() {
        let number_of_line = index + 1;
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        let mut tokens = line.split_whitespace();
        let first = tokens.next().unwrap_or("");

        if first == "pad" {
            let width = number(tokens.next().unwrap_or(""), "the width", number_of_line)?;
            let height = number(tokens.next().unwrap_or(""), "the height", number_of_line)?;
            pad = Some(Pad {
                width_cm: width,
                height_cm: height,
            });
            continue;
        }

        let time_ms = number(first, "the time", number_of_line)?;
        if time_ms < 0.0 {
            return Err(format!(
                "line {number_of_line}: the time cannot be negative"
            ));
        }
        let time = Duration::from_secs_f64(time_ms / 1000.0);

        if frames.last().is_some_and(|last| time < last.time) {
            return Err(format!(
                "line {number_of_line}: the time goes backwards, from {:.1} to {time_ms:.1}",
                millis(frames.last().map_or(Duration::ZERO, |last| last.time))
            ));
        }

        let mut contacts = vec![];
        for finger in tokens {
            let parts: Vec<&str> = finger.split(':').collect();
            let confident = match parts.as_slice() {
                [_, _, _] => true,
                [_, _, _, "palm"] => false,
                _ => {
                    return Err(format!(
                        "line {number_of_line}: a finger is id:x:y (or id:x:y:palm), not `{finger}`"
                    ))
                }
            };

            contacts.push(Contact {
                id: parts[0].parse::<u32>().map_err(|_| {
                    format!(
                        "line {number_of_line}: the id should be a whole number, not `{}`",
                        parts[0]
                    )
                })?,
                x_cm: number(parts[1], "x", number_of_line)?,
                y_cm: number(parts[2], "y", number_of_line)?,
                confident,
            });
        }

        frames.push(Frame { time, contacts });
    }

    Ok(Recording { pad, frames })
}

/// How long a frame takes: the touchpad of a laptop reports about 125 times a second
const FRAME: Duration = Duration::from_millis(8);

/// Where the time of a made-up gesture starts, so that it looks like the clock of a device
const START: Duration = Duration::from_millis(100_000);

/// The names of the made-up gestures
pub const DEMOS: [&str; 12] = [
    "swipe3left",
    "swipe3right",
    "swipe3up",
    "swipe3down",
    "swipe4left",
    "swipe4right",
    "swipe4up",
    "swipe4down",
    "pan3-slow",
    "pan3-flick",
    "pan3-long",
    "scroll-then-three",
];

/// How fingers move
#[derive(Copy, Clone)]
enum Motion {
    /// Slowly, then fast, then slowly again, so they lift at a crawl (about 1 cm/s)
    Eased,
    /// At a steady speed, so they are still going at the end: how a flick lifts
    Steady,
}

/// `fingers` a centimetre apart, going from where they were by (`dx`, `dy`) over `milliseconds`
fn drag(
    frames: &mut Vec<Frame>,
    time: &mut Duration,
    motion: Motion,
    fingers: u32,
    from: (f64, f64),
    by: (f64, f64),
    milliseconds: u64,
) {
    let steps = (milliseconds / 8).max(1);
    for step in 1..=steps {
        let t = step as f64 / steps as f64;
        let eased = match motion {
            // smoothstep: slow, fast, slow
            Motion::Eased => t * t * (3.0 - 2.0 * t),
            Motion::Steady => t,
        };
        *time += FRAME;
        frames.push(hand(
            *time,
            fingers,
            from.0 + by.0 * eased,
            from.1 + by.1 * eased,
        ));
    }
}

fn hand(time: Duration, fingers: u32, x: f64, y: f64) -> Frame {
    Frame {
        time,
        contacts: (0..fingers)
            .map(|i| Contact {
                id: i,
                x_cm: x + f64::from(i),
                y_cm: y,
                confident: true,
            })
            .collect(),
    }
}

fn rest(
    frames: &mut Vec<Frame>,
    time: &mut Duration,
    fingers: u32,
    at: (f64, f64),
    milliseconds: u64,
) {
    for _ in 0..(milliseconds / 8).max(1) {
        *time += FRAME;
        frames.push(hand(*time, fingers, at.0, at.1));
    }
}

/// Make the time between frames uneven, like a real touchpad's: each frame is up to 1.5 ms early or
/// late, and they still only go forward. The same every time, so that a recording is repeatable
fn jitter(frames: &mut [Frame]) {
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut previous = Duration::ZERO;

    for frame in frames {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let micros = (seed >> 33) % 3_001;
        let moved = frame.time + Duration::from_micros(micros);
        let moved = moved.saturating_sub(Duration::from_micros(1_500));

        // never before the last frame
        frame.time = moved.max(previous + Duration::from_micros(500));
        previous = frame.time;
    }
}

/// A made-up gesture: fingers landing, going somewhere, and lifting. `None` if there is no such
/// gesture
#[must_use]
pub fn demo(name: &str) -> Option<Recording> {
    let mut frames = vec![];
    let mut time = START;
    let middle = (4.0, 3.0);

    // (fingers, how far across and down, how long it takes)
    let swipe = |name: &str| -> Option<(u32, (f64, f64), u64)> {
        let fingers = if name.starts_with("swipe3") { 3 } else { 4 };
        let by = match name
            .trim_start_matches("swipe3")
            .trim_start_matches("swipe4")
        {
            "left" => (-3.0, 0.0),
            "right" => (3.0, 0.0),
            "up" => (0.0, -2.0),
            "down" => (0.0, 2.0),
            _ => return None,
        };
        Some((fingers, by, 240))
    };

    match name {
        "pan3-slow" => {
            // a slow drag that does not get far enough to be a swipe
            rest(&mut frames, &mut time, 3, middle, 80);
            drag(
                &mut frames,
                &mut time,
                Motion::Eased,
                3,
                middle,
                (1.1, 0.1),
                600,
            );
        }
        "pan3-flick" => {
            // a short, quick flick that is a swipe, with the hand carrying on a little
            rest(&mut frames, &mut time, 3, middle, 80);
            drag(
                &mut frames,
                &mut time,
                Motion::Steady,
                3,
                middle,
                (4.0, 0.0),
                120,
            );
        }
        "pan3-long" => {
            // two seconds of a hand that drags one way, back a little, and flicks away, with the
            // timing of a real touchpad: about 125 frames a second, and never exactly
            rest(&mut frames, &mut time, 3, middle, 80);
            drag(
                &mut frames,
                &mut time,
                Motion::Eased,
                3,
                middle,
                (5.0, 0.3),
                1000,
            );
            drag(
                &mut frames,
                &mut time,
                Motion::Eased,
                3,
                (9.0, 3.3),
                (-2.0, -0.2),
                600,
            );
            drag(
                &mut frames,
                &mut time,
                Motion::Steady,
                3,
                (7.0, 3.1),
                (3.2, 0.0),
                200,
            );
            jitter(&mut frames);
        }
        "scroll-then-three" => {
            // two fingers scrolling, and a third joining late: not a gesture
            rest(&mut frames, &mut time, 2, middle, 400);
            rest(&mut frames, &mut time, 3, middle, 80);
            drag(
                &mut frames,
                &mut time,
                Motion::Eased,
                3,
                middle,
                (3.0, 0.0),
                240,
            );
        }
        other => {
            let (fingers, by, duration) = swipe(other)?;
            rest(&mut frames, &mut time, fingers, middle, 80);
            drag(
                &mut frames,
                &mut time,
                Motion::Eased,
                fingers,
                middle,
                by,
                duration,
            );
        }
    }

    // and the fingers lift
    time += FRAME;
    frames.push(Frame {
        time,
        contacts: vec![],
    });

    Some(Recording {
        pad: Some(DEMO_PAD),
        frames,
    })
}

/// Put `recording` through a recogniser, handing what it says to `out`. With `realtime`, waits so
/// that the frames come at the pace they were recorded at, which is what a command reading them
/// would see from a real touchpad
pub fn replay(
    recording: &Recording,
    thresholds: Thresholds,
    realtime: bool,
    mut out: impl FnMut(&Output, Option<Pad>),
) {
    let mut recogniser = Recogniser::new(thresholds);
    let started = Instant::now();
    let first = recording.frames.first().map(|frame| frame.time);

    for frame in &recording.frames {
        if realtime {
            let due = frame.time.saturating_sub(first.unwrap_or(Duration::ZERO));
            if let Some(wait) = due.checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
        }

        for output in recogniser.frame(frame) {
            out(&output, recording.pad);
        }
    }

    // a recording that stops with the fingers still down: the gesture is abandoned
    if let Some(output) = recogniser.reset() {
        out(&output, recording.pad);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whkd_core::GestureInput;

    fn run(name: &str) -> Vec<Output> {
        let recording = demo(name).unwrap_or_else(|| panic!("no demo {name}"));
        let mut outputs = vec![];
        replay(&recording, Thresholds::default(), false, |output, _| {
            outputs.push(*output);
        });
        outputs
    }

    fn swipes(outputs: &[Output]) -> Vec<GestureInput> {
        outputs
            .iter()
            .filter_map(|output| match output {
                Output::Swipe(gesture) => Some(*gesture),
                _ => None,
            })
            .collect()
    }

    fn count(outputs: &[Output], wanted: fn(&Output) -> bool) -> usize {
        outputs.iter().filter(|output| wanted(output)).count()
    }

    #[test]
    fn a_recording_is_written_and_read_back() {
        let recording = demo("swipe3right").unwrap();
        let text = format(&recording);
        let back = parse(&text).unwrap();

        assert_eq!(back.pad, recording.pad);
        assert_eq!(back.frames.len(), recording.frames.len());
        for (a, b) in back.frames.iter().zip(&recording.frames) {
            assert!((millis(a.time) - millis(b.time)).abs() < 0.06);
            assert_eq!(a.contacts.len(), b.contacts.len());
            for (c, d) in a.contacts.iter().zip(&b.contacts) {
                assert_eq!(c.id, d.id);
                assert!((c.x_cm - d.x_cm).abs() < 0.0006 && (c.y_cm - d.y_cm).abs() < 0.0006);
            }
        }
    }

    #[test]
    fn a_file_can_be_written_by_hand() {
        let recording = parse(
            "# a capture\npad 11.7 7.5\n\n100000.0 1:4.0:3.0 2:5.0:3.0 3:6.0:3.0:palm # a palm\n100008.0\n",
        )
        .unwrap();

        assert_eq!(recording.pad, Some(DEMO_PAD));
        assert_eq!(recording.frames.len(), 2);
        assert_eq!(recording.frames[0].contacts.len(), 3);
        assert!(recording.frames[0].contacts[1].confident);
        assert!(!recording.frames[0].contacts[2].confident);
        assert!(recording.frames[1].contacts.is_empty());
    }

    #[test]
    fn mistakes_in_a_file_are_said_with_the_line() {
        for (text, expected) in [
            ("soon 1:2:3\n", "line 1: the time should be a number"),
            ("0 1:2\n", "line 1: a finger is id:x:y"),
            ("0 x:2:3\n", "line 1: the id should be a whole number"),
            ("0 1:two:3\n", "line 1: x should be a number"),
            ("0 1:2:3:thumb\n", "line 1: a finger is id:x:y"),
            ("pad wide 7\n", "line 1: the width should be a number"),
            ("10 1:2:3\n5 1:2:3\n", "line 2: the time goes backwards"),
            ("-1 1:2:3\n", "line 1: the time cannot be negative"),
        ] {
            let error = parse(text).unwrap_err();
            assert!(error.starts_with(expected), "{text:?}: {error}");
        }
    }

    #[test]
    fn every_demo_exists_and_no_other_does() {
        for name in DEMOS {
            let recording = demo(name).unwrap_or_else(|| panic!("{name}"));
            assert!(!recording.frames.is_empty());
            assert_eq!(recording.pad, Some(DEMO_PAD));
            // and the fingers lift at the end
            assert!(recording.frames.last().unwrap().contacts.is_empty());
        }
        assert!(demo("swipe5left").is_none());
        assert!(demo("").is_none());
    }

    #[test]
    fn the_demo_swipes_are_the_swipes_they_are_named_for() {
        for gesture in GestureInput::ALL.into_iter().filter(|g| !g.is_stream()) {
            let outputs = run(gesture.name());
            assert_eq!(swipes(&outputs), vec![gesture], "{gesture}");

            // and it is a whole gesture, streamed
            assert_eq!(
                count(&outputs, |o| matches!(o, Output::Begin { .. })),
                1,
                "{gesture}"
            );
            assert_eq!(
                count(&outputs, |o| matches!(o, Output::End { .. })),
                1,
                "{gesture}"
            );
            assert_eq!(
                count(&outputs, |o| matches!(o, Output::Cancel)),
                0,
                "{gesture}"
            );
            assert!(
                count(&outputs, |o| matches!(o, Output::Move { .. })) > 5,
                "{gesture}"
            );
        }
    }

    #[test]
    fn a_slow_pan_is_streamed_without_being_a_swipe() {
        let outputs = run("pan3-slow");
        assert!(swipes(&outputs).is_empty());
        assert_eq!(
            count(&outputs, |o| matches!(o, Output::Begin { fingers: 3, .. })),
            1
        );
        assert!(count(&outputs, |o| matches!(o, Output::Move { .. })) > 20);
        assert_eq!(count(&outputs, |o| matches!(o, Output::End { .. })), 1);
    }

    #[test]
    fn a_flick_is_a_swipe_and_a_stream() {
        let outputs = run("pan3-flick");
        assert_eq!(swipes(&outputs), vec![GestureInput::Swipe3Right]);
    }

    #[test]
    fn a_third_finger_joining_a_scroll_is_nothing() {
        assert!(run("scroll-then-three").is_empty());
    }

    #[test]
    fn a_recording_that_stops_with_the_fingers_down_is_cancelled() {
        let mut recording = demo("swipe3right").unwrap();
        // without the lift at the end
        recording.frames.pop();

        let mut outputs = vec![];
        replay(&recording, Thresholds::default(), false, |output, _| {
            outputs.push(*output);
        });

        assert_eq!(outputs.last(), Some(&Output::Cancel));
        assert_eq!(count(&outputs, |o| matches!(o, Output::End { .. })), 0);
    }

    #[test]
    fn realtime_keeps_the_pace_of_the_recording() {
        let mut recording = demo("swipe3right").unwrap();
        // 10 frames of 8 ms
        recording.frames.truncate(10);

        let start = Instant::now();
        replay(&recording, Thresholds::default(), true, |_, _| {});
        let took = start.elapsed();

        assert!(took >= Duration::from_millis(60), "{took:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
    }

    #[test]
    fn the_long_pan_has_the_timing_of_a_real_touchpad() {
        let recording = demo("pan3-long").unwrap();
        let times: Vec<f64> = recording.frames.iter().map(|f| millis(f.time)).collect();

        // two seconds, give or take
        let length = times.last().unwrap() - times.first().unwrap();
        assert!((1_500.0..3_000.0).contains(&length), "{length}");

        // about 125 frames a second, but not exactly: the gaps vary, and only ever go forward
        let gaps: Vec<f64> = times.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(gaps.iter().all(|gap| *gap > 0.0));
        let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
        assert!((7.0..9.0).contains(&mean), "{mean}");
        let (shortest, longest) = gaps.iter().fold((f64::MAX, 0.0_f64), |(lo, hi), gap| {
            (lo.min(*gap), hi.max(*gap))
        });
        assert!(longest - shortest > 1.0, "{shortest} {longest}");

        // and the same every time
        assert_eq!(recording, demo("pan3-long").unwrap());
    }

    /// How fast the fingers were going over the last few moves, in cm/s, as a consumer would work it
    /// out from the lines
    fn speed_at_the_end(outputs: &[Output]) -> f64 {
        let moves: Vec<(f64, f64)> = outputs
            .iter()
            .filter_map(|output| match output {
                Output::Move { dx_cm, time, .. } => Some((*dx_cm, millis(*time))),
                _ => None,
            })
            .collect();
        let last = moves[moves.len() - 6..].to_vec();
        let distance: f64 = last[1..].iter().map(|(dx, _)| dx).sum();
        distance / ((last[last.len() - 1].1 - last[0].1) / 1000.0)
    }

    #[test]
    fn a_flick_lifts_at_speed_and_a_slow_pan_does_not() {
        // 4 cm in 120 ms is about 33 cm/s, and the long pan's last stretch is 3.2 cm in 200 ms
        let flick = speed_at_the_end(&run("pan3-flick"));
        assert!((25.0..40.0).contains(&flick), "{flick}");

        let long = speed_at_the_end(&run("pan3-long"));
        assert!((10.0..22.0).contains(&long), "{long}");

        // a pan that eases to a stop lifts at a crawl
        let slow = speed_at_the_end(&run("pan3-slow"));
        assert!(slow.abs() < 3.0, "{slow}");
    }

    #[test]
    fn the_long_pan_is_a_stream_that_ends_in_a_swipe() {
        let outputs = run("pan3-long");
        assert_eq!(swipes(&outputs), vec![GestureInput::Swipe3Right]);
        assert!(count(&outputs, |o| matches!(o, Output::Move { .. })) > 150);
        assert_eq!(count(&outputs, |o| matches!(o, Output::End { .. })), 1);
    }
}
