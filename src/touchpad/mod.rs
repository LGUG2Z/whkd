//! Touchpad gestures.
//!
//! Windows does not pass three and four finger gestures on to programs, but a Precision Touchpad
//! reports where every finger is, and that is enough to recognise them. This is the part that does
//! the recognising: it takes frames (where the fingers are, in centimetres) and says what the
//! fingers did. It has no Win32 in it, so it can be tested with made-up frames.

pub mod device;
pub mod dispatch;
pub mod frame;
pub mod protocol;
pub mod recogniser;
pub mod report;
pub mod stream;
