//! The lines a stream binding's command reads on its standard input. Written up for the people who
//! write those commands in `docs/gesture-stream.md`, which is what this has to agree with.

use super::recogniser::Output;

/// The value of `WHKD_GESTURE_PROTOCOL`, for a program that wants to know which version of the
/// format it is reading
pub const VERSION: &str = "1";

/// The size of the touchpad, in centimetres, when it is known
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Pad {
    pub width_cm: f64,
    pub height_cm: f64,
}

/// A distance in centimetres as the format spells it: a decimal point and three decimals
fn cm(value: f64) -> String {
    // no `-0.000`
    let value = if value.abs() < 0.0005 { 0.0 } else { value };
    format!("{value:.3}")
}

fn millis(time: std::time::Duration) -> u64 {
    u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
}

/// The line to write for something the recogniser said, if it is something a stream is told: a swipe
/// that runs a command once is not
#[must_use]
pub fn encode(output: &Output, pad: Option<Pad>) -> Option<String> {
    match output {
        Output::Begin {
            fingers,
            x_cm,
            y_cm,
            time,
        } => {
            let size = pad.map_or_else(String::new, |pad| {
                format!(" w={} h={}", cm(pad.width_cm), cm(pad.height_cm))
            });

            Some(format!(
                "begin fingers={fingers} x={} y={}{size} t={}",
                cm(*x_cm),
                cm(*y_cm),
                millis(*time)
            ))
        }
        Output::Move {
            dx_cm,
            dy_cm,
            x_cm,
            y_cm,
            time,
        } => Some(format!(
            "move dx={} dy={} x={} y={} t={}",
            cm(*dx_cm),
            cm(*dy_cm),
            cm(*x_cm),
            cm(*y_cm),
            millis(*time)
        )),
        Output::End { x_cm, y_cm, time } => Some(format!(
            "end x={} y={} t={}",
            cm(*x_cm),
            cm(*y_cm),
            millis(*time)
        )),
        Output::Cancel => Some(String::from("cancel")),
        Output::Swipe(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;
    use whkd_core::GestureInput;

    const TRANSCRIPT: &str = include_str!("../../docs/gesture-stream.transcript.txt");

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// The example in the documentation, as what the recogniser would say
    fn example() -> Vec<Output> {
        vec![
            Output::Begin {
                fingers: 3,
                x_cm: 4.0,
                y_cm: 2.0,
                time: ms(118_337),
            },
            Output::Move {
                dx_cm: 0.42,
                dy_cm: -0.03,
                x_cm: 4.42,
                y_cm: 1.97,
                time: ms(118_345),
            },
            Output::Move {
                dx_cm: 0.51,
                dy_cm: -0.02,
                x_cm: 4.93,
                y_cm: 1.95,
                time: ms(118_353),
            },
            Output::Move {
                dx_cm: 0.47,
                dy_cm: 0.0,
                x_cm: 5.4,
                y_cm: 1.95,
                time: ms(118_361),
            },
            Output::End {
                x_cm: 5.4,
                y_cm: 1.95,
                time: ms(118_369),
            },
        ]
    }

    #[test]
    fn the_documented_example_is_what_is_written() {
        let pad = Some(Pad {
            width_cm: 10.5,
            height_cm: 7.2,
        });

        let written: String = example()
            .iter()
            .filter_map(|output| encode(output, pad))
            .map(|line| format!("{line}\n"))
            .collect();

        assert_eq!(written, TRANSCRIPT.replace("\r\n", "\n"));
    }

    #[test]
    fn the_size_of_the_pad_is_left_out_when_it_is_not_known() {
        let line = encode(&example()[0], None).unwrap();
        assert_eq!(line, "begin fingers=3 x=4.000 y=2.000 t=118337");
    }

    #[test]
    fn numbers_have_a_point_and_three_decimals_and_never_negative_zero() {
        assert_eq!(cm(0.0), "0.000");
        assert_eq!(cm(-0.0), "0.000");
        assert_eq!(cm(-0.0001), "0.000");
        assert_eq!(cm(0.4), "0.400");
        assert_eq!(cm(-1.23456), "-1.235");
        assert_eq!(cm(12.0), "12.000");
    }

    #[test]
    fn cancel_has_no_fields_and_a_swipe_is_not_written() {
        assert_eq!(encode(&Output::Cancel, None).as_deref(), Some("cancel"));
        assert_eq!(encode(&Output::Swipe(GestureInput::Swipe3Left), None), None);
    }

    /// A reader of the format, as the documentation describes it, to check that what is written
    /// can be read that way
    fn read(line: &str) -> (&str, HashMap<&str, &str>) {
        let mut parts = line.split(' ');
        let name = parts.next().unwrap();
        let fields = parts
            .map(|field| field.split_once('=').expect("key=value"))
            .collect();
        (name, fields)
    }

    #[test]
    fn what_is_written_follows_the_rules_of_the_format() {
        let pad = Some(Pad {
            width_cm: 10.5,
            height_cm: 7.2,
        });
        let lines: Vec<String> = example()
            .iter()
            .filter_map(|output| encode(output, pad))
            .collect();

        let names: Vec<&str> = lines.iter().map(|line| read(line).0).collect();
        assert_eq!(names, vec!["begin", "move", "move", "move", "end"]);

        let mut last = 0;
        for line in &lines {
            let (_, fields) = read(line);
            // every number parses, and times only go up
            let t: u64 = fields["t"].parse().unwrap();
            assert!(t >= last);
            last = t;
            for (key, value) in &fields {
                if *key != "fingers" && *key != "t" {
                    assert!(value.parse::<f64>().is_ok(), "{key}={value}");
                    // a point and three decimals
                    assert_eq!(
                        value.split('.').nth(1).map(str::len),
                        Some(3),
                        "{key}={value}"
                    );
                }
            }
        }

        // the moves add up to how far the middle of the fingers went
        let dx: f64 = lines
            .iter()
            .map(|line| read(line))
            .filter(|(name, _)| *name == "move")
            .map(|(_, fields)| fields["dx"].parse::<f64>().unwrap())
            .sum();
        assert!((dx - 1.4).abs() < 1e-9);
    }
}
