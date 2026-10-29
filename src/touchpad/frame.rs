//! What a touchpad reports.

use std::time::Duration;

/// One finger on the touchpad
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Contact {
    /// Identifies the finger for as long as it stays down
    pub id: u32,
    /// Where it is, in centimetres from the touchpad's top left corner: x grows to the right and y
    /// downwards
    pub x_cm: f64,
    pub y_cm: f64,
    /// Whether the touchpad believes this is a finger. A palm resting on it is not, and is ignored
    pub confident: bool,
}

/// Where every finger that is touching the touchpad is, at one moment
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// When, from any starting point: only the differences between frames matter
    pub time: Duration,
    pub contacts: Vec<Contact>,
}

#[cfg(test)]
impl Frame {
    /// A frame for tests: `fingers` are `(id, x, y)`, all confident
    #[must_use]
    pub fn new(time: Duration, fingers: &[(u32, f64, f64)]) -> Self {
        Self {
            time,
            contacts: fingers
                .iter()
                .map(|&(id, x_cm, y_cm)| Contact {
                    id,
                    x_cm,
                    y_cm,
                    confident: true,
                })
                .collect(),
        }
    }
}
