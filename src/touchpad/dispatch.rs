//! What to do about what the recogniser says: start a stream for `swipe3` and `swipe4`, and run a
//! command once for a swipe that goes one way.
//!
//! Which command a binding runs can depend on the focused application and on komorebi, and
//! finding out can take a moment, so this runs on a thread of its own and the thread that reads
//! the touchpad only queues what it sees. Nothing is lost while it works out the command: the
//! movements that came in meanwhile are waiting, and the stream writer combines them if there are
//! too many.

use super::protocol::Pad;
use super::recogniser::Output;
use super::stream::Stream;
use crate::mouse::Entries;
use crate::mouse::Modifiers;
use std::collections::HashMap;
use whkd_core::GestureInput;
use whkd_core::Shell;

/// Which command the entries of a binding come to, for what is going on now
type Choose = Box<dyn Fn(&Entries) -> Option<String> + Send>;

pub struct Dispatcher {
    bindings: HashMap<(GestureInput, Modifiers), Entries>,
    shell: Shell,
    choose: Choose,
    /// Run a command once
    run_once: Box<dyn Fn(&str) + Send>,
    /// The gesture that is being told to a command, if there is one
    current: Option<Stream>,
}

impl Dispatcher {
    pub fn new(
        bindings: impl IntoIterator<Item = ((GestureInput, Modifiers), Entries)>,
        shell: Shell,
        choose: impl Fn(&Entries) -> Option<String> + Send + 'static,
        run_once: impl Fn(&str) + Send + 'static,
    ) -> Self {
        Self {
            bindings: bindings.into_iter().collect(),
            shell,
            choose: Box::new(choose),
            run_once: Box::new(run_once),
            current: None,
        }
    }

    /// Act on `output`, with `modifiers` held now, for a touchpad that is `pad` big
    pub fn handle(&mut self, output: Output, modifiers: Modifiers, pad: Option<Pad>) {
        match output {
            Output::Begin { fingers, .. } => {
                // a gesture that never ended (a touchpad that went away, say) is over now
                self.current = None;

                let command = GestureInput::stream(fingers)
                    .and_then(|input| self.bindings.get(&(input, modifiers)))
                    .and_then(|entries| (self.choose)(entries));

                if let Some(command) = command {
                    match Stream::start(self.shell, &command, pad) {
                        Ok(mut stream) => {
                            stream.send(output);
                            self.current = Some(stream);
                        }
                        Err(error) => {
                            eprintln!("Unable to start '{command}' for a gesture (error: {error})");
                        }
                    }
                }
            }
            Output::Move { .. } => {
                if let Some(stream) = &mut self.current {
                    stream.send(output);
                }
            }
            Output::End { .. } | Output::Cancel => {
                if let Some(mut stream) = self.current.take() {
                    stream.send(output);
                }
            }
            Output::Swipe(gesture) => {
                let command = self
                    .bindings
                    .get(&(gesture, modifiers))
                    .and_then(|entries| (self.choose)(entries));

                if let Some(command) = command {
                    (self.run_once)(&command);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HkmData;
    use crate::Trigger;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;
    use std::time::Instant;
    use whkd_core::Condition;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn entries(input: GestureInput, command: &str) -> Entries {
        Arc::new(vec![HkmData {
            trigger: Trigger::Gesture {
                mod_keys: vec![],
                input,
            },
            command: command.to_string(),
            condition: Condition::Always,
        }])
    }

    fn first_command(entries: &Entries) -> Option<String> {
        entries.first().map(|entry| entry.command.clone())
    }

    fn begin(fingers: u8) -> Output {
        Output::Begin {
            fingers,
            x_cm: 4.0,
            y_cm: 2.0,
            time: ms(1000),
        }
    }

    fn mv(time: u64) -> Output {
        Output::Move {
            dx_cm: 0.5,
            dy_cm: 0.0,
            x_cm: 4.5,
            y_cm: 2.0,
            time: ms(time),
        }
    }

    fn end() -> Output {
        Output::End {
            x_cm: 4.5,
            y_cm: 2.0,
            time: ms(1100),
        }
    }

    /// A dispatcher that records the commands it runs once
    fn dispatcher(
        bindings: Vec<((GestureInput, Modifiers), Entries)>,
    ) -> (Dispatcher, Arc<Mutex<Vec<String>>>) {
        let ran = Arc::new(Mutex::new(vec![]));
        let record = Arc::clone(&ran);
        let dispatcher = Dispatcher::new(bindings, Shell::Cmd, first_command, move |command| {
            record.lock().unwrap().push(command.to_string());
        });
        (dispatcher, ran)
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("whkd-dispatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    fn wait_for(path: &Path, done: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            if done(&text) || Instant::now() > deadline {
                return text;
            }
            std::thread::sleep(ms(50));
        }
    }

    #[test]
    fn a_swipe_runs_its_command_once_for_the_modifiers_that_are_held() {
        let none = Modifiers::NONE;
        let (mut d, ran) = dispatcher(vec![
            (
                (GestureInput::Swipe3Left, none),
                entries(GestureInput::Swipe3Left, "left"),
            ),
            (
                (GestureInput::Swipe3Left, Modifiers::ALT),
                entries(GestureInput::Swipe3Left, "alt left"),
            ),
        ]);

        d.handle(Output::Swipe(GestureInput::Swipe3Left), none, None);
        d.handle(
            Output::Swipe(GestureInput::Swipe3Left),
            Modifiers::ALT,
            None,
        );
        // not bound with these, and not bound at all
        d.handle(
            Output::Swipe(GestureInput::Swipe3Left),
            Modifiers::CTRL,
            None,
        );
        d.handle(Output::Swipe(GestureInput::Swipe3Right), none, None);

        assert_eq!(*ran.lock().unwrap(), vec!["left", "alt left"]);
    }

    #[test]
    fn a_swipe_does_not_run_a_command_when_there_is_none_to_choose() {
        let ran = Arc::new(Mutex::new(vec![]));
        let record = Arc::clone(&ran);
        let mut d = Dispatcher::new(
            [(
                (GestureInput::Swipe4Up, Modifiers::NONE),
                entries(GestureInput::Swipe4Up, "x"),
            )],
            Shell::Cmd,
            // no condition matched
            |_| None,
            move |command| record.lock().unwrap().push(command.to_string()),
        );

        d.handle(Output::Swipe(GestureInput::Swipe4Up), Modifiers::NONE, None);
        assert!(ran.lock().unwrap().is_empty());
    }

    #[test]
    fn a_stream_starts_one_command_and_is_told_the_whole_gesture() {
        let out = scratch("stream.txt");
        let (mut d, ran) = dispatcher(vec![(
            (GestureInput::Swipe3, Modifiers::NONE),
            entries(
                GestureInput::Swipe3,
                &format!("findstr \"^\" > \"{}\"", out.display()),
            ),
        )]);

        let pad = Some(Pad {
            width_cm: 10.5,
            height_cm: 7.2,
        });
        d.handle(begin(3), Modifiers::NONE, pad);
        std::thread::sleep(ms(100));
        d.handle(mv(1008), Modifiers::NONE, pad);
        std::thread::sleep(ms(100));
        d.handle(mv(1016), Modifiers::NONE, pad);
        std::thread::sleep(ms(100));
        d.handle(end(), Modifiers::NONE, pad);

        let text = wait_for(&out, |text| text.contains("end "));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text:?}");
        assert!(lines[0].starts_with("begin fingers=3 x=4.000 y=2.000 w=10.500 h=7.200"));
        assert!(lines[3].starts_with("end "));
        // a stream is not run once
        assert!(ran.lock().unwrap().is_empty());
    }

    #[test]
    fn a_gesture_with_no_stream_bound_for_its_fingers_starts_nothing() {
        let out = scratch("none.txt");
        let (mut d, _) = dispatcher(vec![(
            (GestureInput::Swipe3, Modifiers::NONE),
            entries(
                GestureInput::Swipe3,
                &format!("findstr \"^\" > \"{}\"", out.display()),
            ),
        )]);

        // four fingers, and three with a modifier held: neither is bound
        d.handle(begin(4), Modifiers::NONE, None);
        d.handle(mv(1008), Modifiers::NONE, None);
        d.handle(end(), Modifiers::NONE, None);
        d.handle(begin(3), Modifiers::ALT, None);
        d.handle(end(), Modifiers::ALT, None);

        std::thread::sleep(ms(500));
        assert!(!out.exists());
    }

    #[test]
    fn a_stream_can_be_chosen_by_what_is_going_on() {
        // the entries of a binding, from which only one command is chosen
        let out = scratch("chosen.txt");
        let ran = Arc::new(Mutex::new(vec![]));
        let record = Arc::clone(&ran);
        let command = format!("findstr \"^\" > \"{}\"", out.display());
        let mut d = Dispatcher::new(
            [(
                (GestureInput::Swipe4, Modifiers::NONE),
                entries(GestureInput::Swipe4, &command),
            )],
            Shell::Cmd,
            |entries| {
                // stands in for choosing by the application and the layout
                entries.last().map(|entry| entry.command.clone())
            },
            move |c| record.lock().unwrap().push(c.to_string()),
        );

        d.handle(begin(4), Modifiers::NONE, None);
        std::thread::sleep(ms(100));
        d.handle(end(), Modifiers::NONE, None);
        let text = wait_for(&out, |text| text.contains("end "));
        assert!(text.starts_with("begin fingers=4"), "{text:?}");
    }

    #[test]
    fn a_gesture_that_began_again_before_it_ended_cancels_the_first() {
        let first = scratch("first.txt");
        let (mut d, _) = dispatcher(vec![(
            (GestureInput::Swipe3, Modifiers::NONE),
            entries(
                GestureInput::Swipe3,
                &format!("findstr \"^\" > \"{}\"", first.display()),
            ),
        )]);

        d.handle(begin(3), Modifiers::NONE, None);
        std::thread::sleep(ms(100));
        // the touchpad went away mid-gesture, and came back with another
        d.handle(begin(3), Modifiers::NONE, None);

        let text = wait_for(&first, |text| text.contains("cancel"));
        assert!(text.contains("cancel"), "{text:?}");
    }
}
