//! From what a Precision Touchpad reports to [`Frame`]s.
//!
//! A touchpad reports its fingers as HID reports of raw numbers. This turns them into centimetres
//! (from what the touchpad says about its own units, never from numbers assumed for one model),
//! puts together the frames that a touchpad sends in several reports, and makes sense of its clock.
//! None of it needs Windows, so it can be tested with made-up reports.

use super::frame::Contact;
use super::frame::Frame;
use super::protocol::Pad;
use std::time::Duration;

/// One finger, as the touchpad reports it: raw numbers
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct RawContact {
    pub id: u32,
    pub x: i64,
    pub y: i64,
    /// Whether it is touching the pad (and not just been lifted)
    pub tip: bool,
    /// Whether the touchpad believes it is a finger and not a palm
    pub confident: bool,
}

/// One HID report: some of the fingers at one moment
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawReport {
    /// The touchpad's clock, in ticks of 100 microseconds, which wraps around
    pub scan_time: u32,
    /// How many fingers there are in this frame, as the first report of a frame says. A touchpad
    /// with more fingers than fit in one report sends the rest in reports that say 0
    pub contact_count: u32,
    /// The fingers in this report: as many as fit, and the ones past the count are not real
    pub contacts: Vec<RawContact>,
}

/// What a touchpad says about one axis: how its raw numbers relate to a distance
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Axis {
    pub logical_min: i64,
    pub logical_max: i64,
    pub physical_min: i64,
    pub physical_max: i64,
    /// The HID unit: `0x11` is centimetres and `0x13` is inches
    pub units: u32,
    /// The power of ten the physical range is in, as the low four bits of a signed number
    pub units_exp: u32,
}

impl Axis {
    /// How many centimetres one step of the physical range is, if the touchpad says in a unit that
    /// is understood
    fn cm_per_physical_unit(&self) -> Option<f64> {
        let exponent = {
            let nibble = (self.units_exp & 0xF) as i32;
            if nibble > 7 {
                nibble - 16
            } else {
                nibble
            }
        };

        let unit = match self.units {
            0x11 => 1.0,
            0x13 => 2.54,
            _ => return None,
        };

        Some(unit * 10f64.powi(exponent))
    }

    /// The length of the axis, in centimetres
    fn extent_cm(&self) -> Option<f64> {
        let per_unit = self.cm_per_physical_unit()?;
        let extent = (self.physical_max - self.physical_min) as f64 * per_unit;
        (extent > 0.0).then_some(extent)
    }

    /// Where a raw value is, in centimetres from the start of the axis
    fn cm(&self, raw: i64) -> f64 {
        let span = (self.logical_max - self.logical_min) as f64;
        let fraction = (raw - self.logical_min) as f64 / span;
        // `extent_cm` has been checked when the calibration was made
        fraction * self.extent_cm().unwrap_or(0.0)
    }
}

/// How to turn a touchpad's raw numbers into centimetres, from what it reports about itself
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Calibration {
    x: Axis,
    y: Axis,
    /// The largest value of the clock, after which it wraps to 0
    scan_time_max: u32,
}

impl Calibration {
    /// `None` if the touchpad does not say its size in a unit that is understood, which means its
    /// movements cannot be told in centimetres
    #[must_use]
    pub fn new(x: Axis, y: Axis, scan_time_max: u32) -> Option<Self> {
        let sensible =
            |axis: &Axis| axis.logical_max > axis.logical_min && axis.extent_cm().is_some();
        (sensible(&x) && sensible(&y)).then_some(Self {
            x,
            y,
            scan_time_max,
        })
    }

    /// How big the touchpad is
    #[must_use]
    pub fn pad(&self) -> Pad {
        Pad {
            width_cm: self.x.extent_cm().unwrap_or(0.0),
            height_cm: self.y.extent_cm().unwrap_or(0.0),
        }
    }

    fn contact(&self, raw: &RawContact) -> Contact {
        Contact {
            id: raw.id,
            x_cm: self.x.cm(raw.x),
            y_cm: self.y.cm(raw.y),
            confident: raw.confident,
        }
    }
}

/// Makes a steadily increasing time of a clock that wraps around
#[derive(Debug)]
struct Clock {
    modulus: u64,
    last: Option<u32>,
    elapsed_ticks: u64,
}

impl Clock {
    fn new(scan_time_max: u32) -> Self {
        // a clock that does not say how far it goes is taken to be 32 bits wide
        let modulus = if scan_time_max == 0 {
            1 << 32
        } else {
            u64::from(scan_time_max) + 1
        };

        Self {
            modulus,
            last: None,
            elapsed_ticks: 0,
        }
    }

    fn time(&mut self, now: u32) -> Duration {
        if let Some(last) = self.last {
            let behind = u64::from(now) % self.modulus;
            let before = u64::from(last) % self.modulus;
            self.elapsed_ticks += (behind + self.modulus - before) % self.modulus;
        }

        self.last = Some(now);
        Duration::from_micros(self.elapsed_ticks * 100)
    }
}

/// Puts the reports of a touchpad together into frames
#[derive(Debug)]
pub struct Assembler {
    calibration: Calibration,
    clock: Clock,
    /// The fingers of the frame that is being put together, and how many it is to have
    pending: Vec<RawContact>,
    expected: usize,
    started: Duration,
}

impl Assembler {
    #[must_use]
    pub fn new(calibration: Calibration) -> Self {
        Self {
            calibration,
            clock: Clock::new(calibration.scan_time_max),
            pending: vec![],
            expected: 0,
            started: Duration::ZERO,
        }
    }

    /// Take a report. When it completes a frame, that is returned
    pub fn push(&mut self, report: &RawReport) -> Option<Frame> {
        let time = self.clock.time(report.scan_time);

        // a report that says how many fingers there are starts a frame. One that says 0 carries
        // on a frame that is not complete, or, when none is, is a frame with no fingers in it
        if report.contact_count > 0 || self.pending.len() >= self.expected {
            self.pending.clear();
            self.expected = report.contact_count as usize;
            self.started = time;
        }

        let missing = self.expected - self.pending.len();
        self.pending
            .extend(report.contacts.iter().take(missing).copied());

        if self.pending.len() < self.expected {
            return None;
        }

        let contacts = self
            .pending
            .drain(..)
            .filter(|raw| raw.tip)
            .map(|raw| self.calibration.contact(&raw))
            .collect();
        self.expected = 0;

        Some(Frame {
            time: self.started,
            contacts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A touchpad 10.0 cm by 6.0 cm, with its coordinates in tenths of a millimetre... here
    /// 1000 by 600 steps, saying so in centimetres with an exponent of -1
    fn cm_axis(logical_max: i64, physical_max: i64) -> Axis {
        Axis {
            logical_min: 0,
            logical_max,
            physical_min: 0,
            physical_max,
            units: 0x11,
            units_exp: 0xF, // -1
        }
    }

    fn calibration() -> Calibration {
        // physical 100 x 60 tenths of a centimetre: 10 cm by 6 cm
        Calibration::new(cm_axis(1000, 100), cm_axis(600, 60), 0xFFFF).unwrap()
    }

    fn finger(id: u32, x: i64, y: i64) -> RawContact {
        RawContact {
            id,
            x,
            y,
            tip: true,
            confident: true,
        }
    }

    fn report(scan_time: u32, contact_count: u32, contacts: &[RawContact]) -> RawReport {
        RawReport {
            scan_time,
            contact_count,
            contacts: contacts.to_vec(),
        }
    }

    #[test]
    fn coordinates_are_converted_with_what_the_touchpad_says_about_itself() {
        let calibration = calibration();
        assert_eq!(calibration.pad().width_cm, 10.0);
        assert!((calibration.pad().height_cm - 6.0).abs() < 1e-12);

        let contact = calibration.contact(&finger(7, 500, 300));
        assert_eq!(contact.id, 7);
        assert!((contact.x_cm - 5.0).abs() < 1e-12);
        assert!((contact.y_cm - 3.0).abs() < 1e-12);
        assert!(contact.confident);
    }

    #[test]
    fn the_unit_and_the_exponent_both_count() {
        // inches, in hundredths: 400 hundredths of an inch is 4 inches, which is 10.16 cm
        let inches = Axis {
            logical_min: 0,
            logical_max: 4000,
            physical_min: 0,
            physical_max: 400,
            units: 0x13,
            units_exp: 0xE, // -2
        };
        let y = cm_axis(600, 60);
        let calibration = Calibration::new(inches, y, 0).unwrap();
        assert!((calibration.pad().width_cm - 10.16).abs() < 1e-9);

        // centimetres with no exponent
        let plain = Axis {
            units_exp: 0,
            physical_max: 10,
            ..cm_axis(1000, 10)
        };
        assert_eq!(plain.extent_cm(), Some(10.0));

        // a positive exponent is a multiple: 2 x 10^1 cm
        let tens = Axis {
            units_exp: 1,
            physical_max: 2,
            ..cm_axis(1000, 2)
        };
        assert_eq!(tens.extent_cm(), Some(20.0));
    }

    #[test]
    fn a_range_that_does_not_start_at_zero_is_measured_from_its_start() {
        let axis = Axis {
            logical_min: 100,
            logical_max: 1100,
            ..cm_axis(1000, 100)
        };
        assert!((axis.cm(100) - 0.0).abs() < 1e-12);
        assert!((axis.cm(600) - 5.0).abs() < 1e-12);
        assert!((axis.cm(1100) - 10.0).abs() < 1e-12);
    }

    #[test]
    fn a_touchpad_that_does_not_say_its_size_cannot_be_used() {
        let unknown_unit = Axis {
            units: 0,
            ..cm_axis(1000, 100)
        };
        assert_eq!(Calibration::new(unknown_unit, cm_axis(600, 60), 0), None);

        let no_physical_range = Axis {
            physical_max: 0,
            ..cm_axis(1000, 100)
        };
        assert_eq!(
            Calibration::new(cm_axis(1000, 100), no_physical_range, 0),
            None
        );

        let no_logical_range = Axis {
            logical_max: 0,
            ..cm_axis(1000, 100)
        };
        assert_eq!(
            Calibration::new(no_logical_range, cm_axis(600, 60), 0),
            None
        );
    }

    #[test]
    fn a_report_with_all_the_fingers_is_a_frame() {
        let mut assembler = Assembler::new(calibration());
        let frame = assembler
            .push(&report(
                10_000,
                3,
                &[
                    finger(1, 100, 100),
                    finger(2, 200, 100),
                    finger(3, 300, 100),
                ],
            ))
            .unwrap();

        assert_eq!(frame.contacts.len(), 3);
        assert_eq!(frame.contacts[2].id, 3);
        assert!((frame.contacts[2].x_cm - 3.0).abs() < 1e-12);
    }

    #[test]
    fn a_frame_in_several_reports_is_one_frame_when_it_is_complete() {
        let mut assembler = Assembler::new(calibration());

        // seven fingers, five to a report: the second says 0 for the count
        let first: Vec<_> = (1..=5)
            .map(|i| finger(i, 100 * i64::from(i), 100))
            .collect();
        let second = [finger(6, 600, 100), finger(7, 700, 100)];

        assert_eq!(assembler.push(&report(10_000, 7, &first)), None);
        let frame = assembler.push(&report(10_000, 0, &second)).unwrap();

        assert_eq!(frame.contacts.len(), 7);
        assert_eq!(frame.contacts[6].id, 7);
    }

    #[test]
    fn the_slots_past_the_count_are_not_fingers() {
        let mut assembler = Assembler::new(calibration());
        // two fingers, and the other three slots hold leftovers
        let slots = [
            finger(1, 100, 100),
            finger(2, 200, 100),
            finger(9, 900, 500),
            finger(9, 900, 500),
            finger(9, 900, 500),
        ];
        let frame = assembler.push(&report(10_000, 2, &slots)).unwrap();
        assert_eq!(frame.contacts.len(), 2);
    }

    #[test]
    fn a_finger_that_has_lifted_is_not_touching() {
        let mut assembler = Assembler::new(calibration());
        let mut lifted = finger(2, 200, 100);
        lifted.tip = false;

        let frame = assembler
            .push(&report(10_000, 2, &[finger(1, 100, 100), lifted]))
            .unwrap();
        assert_eq!(frame.contacts.len(), 1);
        assert_eq!(frame.contacts[0].id, 1);
    }

    #[test]
    fn a_report_with_no_fingers_is_a_frame_with_none() {
        let mut assembler = Assembler::new(calibration());
        assembler
            .push(&report(10_000, 1, &[finger(1, 100, 100)]))
            .unwrap();

        let frame = assembler.push(&report(10_080, 0, &[])).unwrap();
        assert!(frame.contacts.is_empty());
    }

    #[test]
    fn a_palm_is_passed_on_as_not_confident() {
        let mut assembler = Assembler::new(calibration());
        let palm = RawContact {
            confident: false,
            ..finger(5, 500, 500)
        };
        let frame = assembler
            .push(&report(10_000, 2, &[finger(1, 100, 100), palm]))
            .unwrap();
        assert!(!frame.contacts[1].confident);
    }

    #[test]
    fn time_is_the_touchpads_clock_in_steps_of_a_hundred_microseconds() {
        let mut assembler = Assembler::new(calibration());
        let at = |assembler: &mut Assembler, scan_time| {
            assembler
                .push(&report(scan_time, 1, &[finger(1, 100, 100)]))
                .unwrap()
                .time
        };

        let first = at(&mut assembler, 40_000);
        let second = at(&mut assembler, 40_080);
        // 80 ticks of 100 microseconds is 8 ms, whatever the clock started at
        assert_eq!(second - first, Duration::from_millis(8));
    }

    #[test]
    fn a_clock_that_wraps_around_keeps_going_forward() {
        // 0xFFFF is the largest it gets, so 65,535 -> 0 is a step of one tick
        let mut assembler = Assembler::new(calibration());
        let mut times = vec![];
        for scan_time in [65_500, 65_530, 20, 60] {
            times.push(
                assembler
                    .push(&report(scan_time, 1, &[finger(1, 100, 100)]))
                    .unwrap()
                    .time,
            );
        }

        for pair in times.windows(2) {
            assert!(pair[1] > pair[0], "{times:?}");
        }
        // 65,530 to 20 is 26 ticks across the wrap, and 20 to 60 is 40
        assert_eq!(times[2] - times[1], Duration::from_micros(2_600));
        assert_eq!(times[3] - times[2], Duration::from_micros(4_000));
    }

    #[test]
    fn a_frame_in_several_reports_has_the_time_of_the_first() {
        let mut assembler = Assembler::new(calibration());
        assembler
            .push(&report(1_000, 1, &[finger(1, 1, 1)]))
            .unwrap();

        let first: Vec<_> = (1..=5).map(|i| finger(i, 100, 100)).collect();
        assert_eq!(assembler.push(&report(1_100, 6, &first)), None);
        let frame = assembler
            .push(&report(1_104, 0, &[finger(6, 100, 100)]))
            .unwrap();

        // 100 ticks after the first frame: 10 ms, not 10.4
        assert_eq!(frame.time, Duration::from_millis(10));
    }
}
