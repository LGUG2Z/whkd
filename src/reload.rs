//! Reloading the configuration when the whkdrc changes.
//!
//! What whkd has is spread over several hooks and threads (the keyboard, the mouse, the touchpad,
//! the shell), each started for the bindings it was given, so a new configuration is read the way the
//! first one was: by starting whkd again. A change is only acted on once it has been checked, so a
//! whkdrc that is half written, or has a mistake in it, leaves whkd as it is, with the mistake
//! reported in the way `whkd validate` does.
//!
//! The new whkd is told which process it is replacing and waits for it to exit before it listens to
//! anything, so that a key is never handled twice, and the old one only exits once the new one has
//! been started. Whether the file has changed, and what to do about it, are pure and tested; starting
//! and waiting are not.

use color_eyre::eyre::Result;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;
use std::time::SystemTime;
use whkd_parser::KeyValidator;

/// How often the file is looked at
const POLL: Duration = Duration::from_millis(500);

/// How long a new whkd waits for the one it replaces to exit
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// The option that tells a whkd which process it replaces
pub const RELOADED_FROM: &str = "--reloaded-from";

/// What a file looked like when it was looked at
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Stamp {
    modified: SystemTime,
    len: u64,
}

impl Stamp {
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;

        Some(Self {
            modified: metadata.modified().ok()?,
            len: metadata.len(),
        })
    }
}

/// Says when a file has changed, and is not still being changed: a change counts once the file looks
/// the same for two looks in a row, so that one that an editor is in the middle of writing is not read
#[derive(Debug)]
pub struct Watcher {
    seen: Option<Stamp>,
    pending: Option<Stamp>,
}

impl Watcher {
    #[must_use]
    pub fn new(seen: Option<Stamp>) -> Self {
        Self {
            seen,
            pending: None,
        }
    }

    /// Whether the file, which looks like `now`, is one that has changed and been left alone
    pub fn changed(&mut self, now: Option<Stamp>) -> bool {
        // a file that is not there is one that is being replaced, which editors do to save it
        let Some(now) = now else {
            return false;
        };

        if Some(now) == self.seen {
            self.pending = None;
            false
        } else if Some(now) == self.pending {
            self.seen = Some(now);
            self.pending = None;
            true
        } else {
            self.pending = Some(now);
            false
        }
    }
}

/// What a changed whkdrc is to be treated as
#[derive(Debug, PartialEq, Eq)]
pub enum Checked {
    /// It is a configuration whkd can run, and what it has to say about it
    Valid { warnings: Vec<String> },
    /// It is not, and why, as it is reported by `whkd validate`
    Invalid(String),
}

/// Read and check a whkdrc the way `whkd validate` does
#[must_use]
pub fn check(path: &Path, keys: &KeyValidator) -> Checked {
    match whkd_parser::load_checked(&path.to_path_buf(), keys) {
        Ok(loaded) => Checked::Valid {
            warnings: loaded.render_warnings(),
        },
        Err(error) => Checked::Invalid(error.to_string()),
    }
}

/// The arguments for the whkd that replaces this one: what this one was given, and which process it
/// is replacing, once
#[must_use]
pub fn child_args(args: impl IntoIterator<Item = OsString>, pid: u32) -> Vec<OsString> {
    let mut args = args.into_iter();
    let mut kept = vec![];
    let option = OsString::from(RELOADED_FROM);

    while let Some(arg) = args.next() {
        if arg == option {
            // and what it was given
            args.next();
        } else if !arg
            .to_string_lossy()
            .starts_with(&format!("{RELOADED_FROM}="))
        {
            kept.push(arg);
        }
    }

    kept.push(option);
    kept.push(OsString::from(pid.to_string()));
    kept
}

/// Start whkd again, with the same options
fn restart() -> Result<()> {
    let args = child_args(std::env::args_os().skip(1), std::process::id());
    Command::new(std::env::current_exe()?).args(args).spawn()?;
    Ok(())
}

/// Wait for the whkd that this one replaces to exit, which is when the keys are free to be listened to
pub fn wait_for_exit(pid: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::OpenProcess;
    use windows::Win32::System::Threading::WaitForSingleObject;
    use windows::Win32::System::Threading::PROCESS_SYNCHRONIZE;

    // a process that is already gone cannot be opened, and there is nothing to wait for
    let Ok(process) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }) else {
        return;
    };

    unsafe {
        WaitForSingleObject(process, EXIT_TIMEOUT.as_millis() as u32);
        let _ = CloseHandle(process);
    }
}

/// Look at `path` from now on, and when it changes to a configuration that can be used, start whkd
/// again and exit. Meant to be started once
pub fn watch(path: &Path, keys: fn() -> KeyValidator) {
    let path = path.to_path_buf();

    thread::spawn(move || {
        let mut watcher = Watcher::new(Stamp::of(&path));

        loop {
            thread::sleep(POLL);

            if !watcher.changed(Stamp::of(&path)) {
                continue;
            }

            match check(&path, &keys()) {
                Checked::Invalid(report) => {
                    eprintln!("{report}");
                    eprintln!(
                        "{}: not reloaded, whkd is still running the configuration it had",
                        path.display()
                    );
                }
                Checked::Valid { warnings } => {
                    for warning in warnings {
                        eprintln!("{warning}");
                    }

                    println!("{}: changed, reloading", path.display());

                    match restart() {
                        Ok(()) => std::process::exit(0),
                        Err(error) => eprintln!(
                            "Unable to start whkd again (error: {error}), so it is still running the configuration it had"
                        ),
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn stamp(seconds: u64, len: u64) -> Option<Stamp> {
        Some(Stamp {
            modified: UNIX_EPOCH + Duration::from_secs(seconds),
            len,
        })
    }

    #[test]
    fn a_file_that_has_not_changed_has_not_changed() {
        let mut watcher = Watcher::new(stamp(1, 10));

        for _ in 0..5 {
            assert!(!watcher.changed(stamp(1, 10)));
        }
    }

    #[test]
    fn a_change_counts_once_the_file_is_left_alone() {
        let mut watcher = Watcher::new(stamp(1, 10));

        // seen once, which might be a file that is being written
        assert!(!watcher.changed(stamp(2, 12)));
        // and still the same, so it has been written
        assert!(watcher.changed(stamp(2, 12)));
        // once
        assert!(!watcher.changed(stamp(2, 12)));
        assert!(!watcher.changed(stamp(2, 12)));
    }

    #[test]
    fn a_file_that_keeps_changing_is_not_read() {
        let mut watcher = Watcher::new(stamp(1, 10));

        assert!(!watcher.changed(stamp(2, 12)));
        assert!(!watcher.changed(stamp(3, 20)));
        assert!(!watcher.changed(stamp(4, 30)));
        assert!(watcher.changed(stamp(4, 30)));
    }

    #[test]
    fn the_length_matters_as_well_as_the_time() {
        let mut watcher = Watcher::new(stamp(1, 10));

        assert!(!watcher.changed(stamp(1, 11)));
        assert!(watcher.changed(stamp(1, 11)));
    }

    #[test]
    fn a_change_that_is_undone_before_it_counts_is_nothing() {
        let mut watcher = Watcher::new(stamp(1, 10));

        assert!(!watcher.changed(stamp(2, 12)));
        assert!(!watcher.changed(stamp(1, 10)));
        assert!(!watcher.changed(stamp(1, 10)));
        // and what it was is not left over to count as the next change
        assert!(!watcher.changed(stamp(2, 12)));
        assert!(watcher.changed(stamp(2, 12)));
    }

    #[test]
    fn a_file_that_is_not_there_is_waited_for() {
        let mut watcher = Watcher::new(stamp(1, 10));

        // an editor that saves by replacing the file
        assert!(!watcher.changed(None));
        assert!(!watcher.changed(None));
        assert!(!watcher.changed(stamp(2, 12)));
        assert!(!watcher.changed(None));
        assert!(watcher.changed(stamp(2, 12)));
    }

    #[test]
    fn a_file_that_was_not_there_to_begin_with_is_a_change_when_it_is() {
        let mut watcher = Watcher::new(None);

        assert!(!watcher.changed(None));
        assert!(!watcher.changed(stamp(1, 10)));
        assert!(watcher.changed(stamp(1, 10)));
    }

    #[test]
    fn the_stamp_of_a_file_changes_when_it_is_written() {
        let path = std::env::temp_dir().join(format!("whkd-stamp-{}.txt", std::process::id()));
        assert_eq!(Stamp::of(&path), None);

        std::fs::write(&path, "one").unwrap();
        let first = Stamp::of(&path).unwrap();
        assert_eq!(Stamp::of(&path), Some(first));

        std::fs::write(&path, "three").unwrap();
        assert_ne!(Stamp::of(&path), Some(first));

        std::fs::remove_file(&path).unwrap();
        assert_eq!(Stamp::of(&path), None);
    }

    fn args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_new_whkd_has_the_arguments_of_the_old_and_its_process() {
        assert_eq!(child_args(args(&[]), 7), args(&["--reloaded-from", "7"]));
        assert_eq!(
            child_args(args(&["--config", "C:\\a b\\whkdrc"]), 7),
            args(&["--config", "C:\\a b\\whkdrc", "--reloaded-from", "7"])
        );
        assert_eq!(
            child_args(args(&["-c", "x", "--no-hotload"]), 12),
            args(&["-c", "x", "--no-hotload", "--reloaded-from", "12"])
        );
    }

    #[test]
    fn a_whkd_that_was_itself_a_reload_does_not_hand_on_what_it_was_told() {
        assert_eq!(
            child_args(args(&["--reloaded-from", "7", "--config", "x"]), 9),
            args(&["--config", "x", "--reloaded-from", "9"])
        );
        assert_eq!(
            child_args(args(&["--config", "x", "--reloaded-from=7"]), 9),
            args(&["--config", "x", "--reloaded-from", "9"])
        );
    }

    fn written(name: &str, contents: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("whkd-reload-{}-{name}.whkdrc", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn a_whkdrc_that_is_fine_is_valid() {
        let path = written("fine", ".shell cmd\nalt + {h,j} : echo {a,b}\n");
        let checked = check(&path, &whkd_keys::validator());
        std::fs::remove_file(&path).unwrap();

        assert_eq!(checked, Checked::Valid { warnings: vec![] });
    }

    #[test]
    fn warnings_are_handed_on() {
        let path = written("warn", ".shell cmd\nalt + h : first\nalt + h : second\n");
        let checked = check(&path, &whkd_keys::validator());
        std::fs::remove_file(&path).unwrap();

        let Checked::Valid { warnings } = checked else {
            panic!("{checked:?}");
        };
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("already has a default command"),
            "{}",
            warnings[0]
        );
    }

    #[test]
    fn a_whkdrc_with_a_mistake_is_reported_as_validate_reports_it() {
        let path = written("broken", ".shell cmd\nalt + retrun : x\n");
        let checked = check(&path, &whkd_keys::validator());
        std::fs::remove_file(&path).unwrap();

        let Checked::Invalid(report) = checked else {
            panic!("{checked:?}");
        };
        assert!(report.contains("unknown key `retrun`"), "{report}");
        assert!(report.contains("did you mean `return`?"), "{report}");
        // and says where
        assert!(report.contains(":2:"), "{report}");
    }

    #[test]
    fn a_whkdrc_that_is_half_written_is_invalid_not_a_crash() {
        for contents in [
            "",
            ".shell",
            ".shell cmd\nalt + h",
            ".shell cmd\nalt + {h,j",
        ] {
            let path = written("half", contents);
            let checked = check(&path, &whkd_keys::validator());
            std::fs::remove_file(&path).unwrap();

            assert!(
                matches!(checked, Checked::Invalid(_)),
                "{contents:?}: {checked:?}"
            );
        }
    }

    #[test]
    fn a_whkdrc_that_is_gone_is_invalid() {
        let path = std::env::temp_dir().join("whkd-reload-nothing-here.whkdrc");
        assert!(matches!(
            check(&path, &whkd_keys::validator()),
            Checked::Invalid(_)
        ));
    }

    #[test]
    fn a_process_that_is_not_there_is_not_waited_for() {
        // the largest process id there can be is far below this
        let start = std::time::Instant::now();
        wait_for_exit(u32::MAX - 1);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_process_that_is_there_is_waited_for_until_it_exits() {
        // about a second, and nothing that needs a console
        let mut child = Command::new("cmd")
            .args(["/c", "ping -n 2 127.0.0.1 >nul"])
            .spawn()
            .unwrap();

        let start = std::time::Instant::now();
        wait_for_exit(child.id());
        let waited = start.elapsed();

        // it has exited by the time that the wait is over
        assert!(child.try_wait().unwrap().is_some());
        assert!(waited >= Duration::from_millis(300), "{waited:?}");
        assert!(waited < EXIT_TIMEOUT, "{waited:?}");
    }
}
