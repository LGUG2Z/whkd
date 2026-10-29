//! Running the command of a stream binding, and telling it what the fingers do.
//!
//! The command is started once, when the gesture begins, and what the fingers do is written to its
//! standard input as the lines `docs/gesture-stream.md` describes, until they lift and the input is
//! closed. That is one process for the whole gesture, not one for each movement.
//!
//! The writing is done on a thread of its own, so that a command that reads slowly can never hold
//! up the thread that reads the touchpad. While it is slow, movements pile up in the queue and are
//! combined into one, so it catches up instead of falling further behind; the beginning and the
//! end of a gesture are never dropped or combined.

use super::protocol::encode;
use super::protocol::Pad;
use super::protocol::VERSION;
use super::recogniser::Output;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use whkd_core::Shell;

/// A command that has not exited this long after its input was closed is not going to, and is
/// stopped so that it does not pile up
const EXIT_GRACE: Duration = Duration::from_secs(10);

/// Combine the movements that follow one another into one: their distances add up, and the latest
/// position and time stand. Everything else is left as it is, in order
pub fn coalesce(outputs: impl IntoIterator<Item = Output>) -> Vec<Output> {
    let mut out: Vec<Output> = vec![];

    for output in outputs {
        match (out.last_mut(), output) {
            (
                Some(Output::Move {
                    dx_cm,
                    dy_cm,
                    x_cm,
                    y_cm,
                    time,
                }),
                Output::Move {
                    dx_cm: more_dx,
                    dy_cm: more_dy,
                    x_cm: new_x,
                    y_cm: new_y,
                    time: new_time,
                },
            ) => {
                *dx_cm += more_dx;
                *dy_cm += more_dy;
                *x_cm = new_x;
                *y_cm = new_y;
                *time = new_time;
            }
            (_, output) => out.push(output),
        }
    }

    out
}

fn command(shell: Shell, command_line: &str) -> Command {
    let mut process = Command::new(shell.to_string());

    match shell {
        Shell::Cmd => {
            // cmd has its own idea of quoting, so the command line is given to it as it is
            process.arg("/C").raw_arg(command_line);
        }
        Shell::Pwsh | Shell::Powershell => {
            process.args(["-NoProfile", "-Command", command_line]);
        }
    }

    process
        .env("WHKD_GESTURE_PROTOCOL", VERSION)
        .stdin(Stdio::piped());
    process
}

/// A gesture that is being told to a command
#[derive(Debug)]
pub struct Stream {
    queue: mpsc::Sender<Output>,
    finished: bool,
}

impl Stream {
    /// Start `command_line` in `shell` and start telling it
    pub fn start(shell: Shell, command_line: &str, pad: Option<Pad>) -> std::io::Result<Self> {
        let mut child = command(shell, command_line).spawn()?;
        let stdin = child.stdin.take();
        let (queue, pending) = mpsc::channel::<Output>();

        thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                'gesture: while let Ok(first) = pending.recv() {
                    // whatever has piled up while this was blocked on a slow reader
                    let batch = coalesce(std::iter::once(first).chain(pending.try_iter()));

                    for output in batch {
                        if let Some(line) = encode(&output, pad) {
                            // one write for the line, and a flush, so that it is seen at once
                            if writeln!(stdin, "{line}")
                                .and_then(|()| stdin.flush())
                                .is_err()
                            {
                                // the command went away
                                break 'gesture;
                            }
                        }

                        if matches!(output, Output::End { .. } | Output::Cancel) {
                            break 'gesture;
                        }
                    }
                }
                // closing the input is how the command knows the gesture is over
                drop(stdin);
            }

            reap(child);
        });

        Ok(Self {
            queue,
            finished: false,
        })
    }

    /// Tell the command what the fingers did. Once the gesture has ended, or been cancelled, nothing
    /// more is told
    pub fn send(&mut self, output: Output) {
        if self.finished {
            return;
        }

        self.finished = matches!(output, Output::End { .. } | Output::Cancel);
        // the writer has gone if the command did
        let _ = self.queue.send(output);
    }
}

impl Drop for Stream {
    /// A gesture that is dropped without having ended (whkd is shutting down, say) is cancelled,
    /// so that the command is not left waiting
    fn drop(&mut self) {
        self.send(Output::Cancel);
    }
}

/// Wait for the command to exit, and stop it if it will not
fn reap(mut child: Child) {
    let deadline = std::time::Instant::now() + EXIT_GRACE;

    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Instant;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn mv(dx: f64, dy: f64, x: f64, t: u64) -> Output {
        Output::Move {
            dx_cm: dx,
            dy_cm: dy,
            x_cm: x,
            y_cm: 2.0,
            time: ms(t),
        }
    }

    #[test]
    fn movements_that_pile_up_are_combined() {
        let combined = coalesce([
            mv(0.1, 0.0, 4.1, 1),
            mv(0.2, -0.1, 4.3, 2),
            mv(0.3, 0.1, 4.6, 3),
        ]);

        assert_eq!(combined.len(), 1);
        match combined[0] {
            Output::Move {
                dx_cm,
                dy_cm,
                x_cm,
                time,
                ..
            } => {
                assert!((dx_cm - 0.6).abs() < 1e-12);
                assert!(dy_cm.abs() < 1e-12);
                // where the fingers are now, and when
                assert_eq!(x_cm, 4.6);
                assert_eq!(time, ms(3));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_beginning_and_the_end_are_never_combined_or_dropped() {
        let begin = Output::Begin {
            fingers: 3,
            x_cm: 4.0,
            y_cm: 2.0,
            time: ms(0),
        };
        let end = Output::End {
            x_cm: 4.6,
            y_cm: 2.0,
            time: ms(4),
        };

        let out = coalesce([
            begin,
            mv(0.1, 0.0, 4.1, 1),
            mv(0.2, 0.0, 4.3, 2),
            mv(0.3, 0.0, 4.6, 3),
            end,
        ]);

        assert_eq!(out.len(), 3);
        assert_eq!(out[0], begin);
        assert!(matches!(out[1], Output::Move { .. }));
        assert_eq!(out[2], end);

        assert_eq!(
            coalesce([begin, Output::Cancel]),
            vec![begin, Output::Cancel]
        );
    }

    #[test]
    fn nothing_is_combined_across_other_messages() {
        let swipe = Output::Swipe(whkd_core::GestureInput::Swipe3Left);
        let out = coalesce([mv(0.1, 0.0, 4.1, 1), swipe, mv(0.2, 0.0, 4.3, 2)]);
        assert_eq!(out.len(), 3);
    }

    fn wait_for(path: &Path, contents: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            if contents(&text) || Instant::now() > deadline {
                return text;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("whkd-stream-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    /// A real process, started once, that reads the gesture from its input and is told when it ends
    #[test]
    fn a_command_is_started_once_and_reads_the_whole_gesture() {
        let out = scratch("gesture.txt");

        // findstr copies its input to its output, which is redirected to the file
        let mut stream = Stream::start(
            Shell::Cmd,
            &format!("findstr \"^\" > \"{}\"", out.display()),
            Some(Pad {
                width_cm: 10.5,
                height_cm: 7.2,
            }),
        )
        .unwrap();

        // paced like a touchpad, so that the writer sees them one at a time. In a burst it would
        // combine the movements, which is what the tests of that are for
        let pace = || thread::sleep(ms(100));
        stream.send(Output::Begin {
            fingers: 3,
            x_cm: 4.0,
            y_cm: 2.0,
            time: ms(118_337),
        });
        pace();
        stream.send(mv(0.42, -0.03, 4.42, 118_345));
        pace();
        stream.send(mv(0.51, -0.02, 4.93, 118_353));
        pace();
        stream.send(Output::End {
            x_cm: 4.93,
            y_cm: 1.95,
            time: ms(118_369),
        });
        // after the end, nothing more is said
        stream.send(mv(9.0, 9.0, 9.0, 118_400));

        let text = wait_for(&out, |text| text.contains("end "));
        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(lines.len(), 4, "{text:?}");
        assert_eq!(
            lines[0],
            "begin fingers=3 x=4.000 y=2.000 w=10.500 h=7.200 t=118337"
        );
        assert!(
            lines[1].starts_with("move dx=0.420 dy=-0.030"),
            "{}",
            lines[1]
        );
        assert_eq!(lines[3], "end x=4.930 y=1.950 t=118369");
    }

    #[test]
    fn the_command_can_tell_which_version_of_the_format_it_is_reading() {
        let out = scratch("version.txt");

        let mut stream = Stream::start(
            Shell::Cmd,
            // (a digit straight before `>` would be cmd redirecting that handle)
            &format!("(echo v=%WHKD_GESTURE_PROTOCOL%) > \"{}\"", out.display()),
            None,
        )
        .unwrap();
        stream.send(Output::Cancel);

        let text = wait_for(&out, |text| !text.trim().is_empty());
        assert_eq!(text.trim(), format!("v={VERSION}"));
    }

    #[test]
    fn a_gesture_that_is_dropped_is_cancelled() {
        let out = scratch("dropped.txt");

        let mut stream = Stream::start(
            Shell::Cmd,
            &format!("findstr \"^\" > \"{}\"", out.display()),
            None,
        )
        .unwrap();
        stream.send(Output::Begin {
            fingers: 4,
            x_cm: 1.0,
            y_cm: 1.0,
            time: ms(1),
        });
        drop(stream);

        let text = wait_for(&out, |text| text.contains("cancel"));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.last().copied(), Some("cancel"), "{text:?}");
    }

    #[test]
    fn a_command_that_does_not_read_does_not_hold_anything_up() {
        // `exit` does not read its input, and the process is gone at once
        let mut stream = Stream::start(Shell::Cmd, "exit 0", None).unwrap();
        thread::sleep(ms(300));

        let start = Instant::now();
        for i in 0..10_000 {
            stream.send(mv(0.01, 0.0, 4.0, i));
        }
        stream.send(Output::Cancel);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
