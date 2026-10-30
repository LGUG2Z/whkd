//! Deciding the conditions that `.when` defines, by running their commands.
//!
//! A command is run in `cmd`, whatever the `.shell` is: it takes about 12 milliseconds to start, where
//! PowerShell takes 150 to 250 (and a profile much more), which is too long to wait for on every key
//! press. On its own a condition holds if the command exits with 0 in time, and compared with a value
//! it is what the command wrote that is compared. The commands that are asked about are run once for
//! each key press. A command that does not finish in time is stopped, with the programs that it
//! started, and nothing can be said about it.
//!
//! Which conditions are asked about, and when, is `whkd_core::select`. What is here is running a
//! command, and remembering what it said for as long as a key press takes.

use parking_lot::Mutex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::Read;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::process::Stdio;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use whkd_core::Checks;
use windows::core::PCWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::JobObjects::AssignProcessToJobObject;
use windows::Win32::System::JobObjects::CreateJobObjectW;
use windows::Win32::System::JobObjects::TerminateJobObject;
use windows::Win32::System::Threading::WaitForSingleObject;

/// How long a command has to decide
pub const TIMEOUT: Duration = Duration::from_millis(500);

/// How much of what a command writes is kept, which is far more than a value to compare it with
const OUTPUT_LIMIT: u64 = 64 * 1024;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What running the command of a condition came to
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It exited, with what it exited with and what it wrote
    Ran { code: i32, output: String },
    /// It did not exit in time, and was stopped
    TimedOut,
    /// It could not be run
    CouldNotRun(String),
}

impl Outcome {
    /// Whether it exited with 0
    #[must_use]
    pub fn holds(&self) -> bool {
        matches!(self, Self::Ran { code: 0, .. })
    }

    /// What it wrote, without the white space around it, if it exited with 0
    #[must_use]
    pub fn says(&self) -> Option<String> {
        match self {
            Self::Ran { code: 0, output } => Some(output.trim().to_string()),
            _ => None,
        }
    }

    /// What to say about it, for the outcomes that are not the condition not holding but something
    /// being wrong
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::TimedOut => Some(format!(
                "did not finish in {} milliseconds, and was stopped, so nothing can be said about it",
                TIMEOUT.as_millis()
            )),
            Self::CouldNotRun(error) => Some(format!(
                "could not be run ({error}), so nothing can be said about it"
            )),
            Self::Ran { .. } => None,
        }
    }
}

/// Run `command` in `cmd`, with the focused application's name in `WHKD_APP`
#[must_use]
pub fn run(command: &str, app: &str, timeout: Duration) -> Outcome {
    // `/s` strips the quotes that go round the command, so that what is in it is read as written
    let spawned = Command::new("cmd")
        .raw_arg("/d /s /c")
        .raw_arg(format!("\"{command}\""))
        .env("WHKD_APP", app)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();

    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return Outcome::CouldNotRun(error.to_string()),
    };

    // what it writes is read as it is written, so that a command that writes a lot is not stopped
    // from finishing by a pipe that is full
    let reader = child.stdout.take().map(|mut stdout| {
        thread::spawn(move || {
            let mut output = vec![];
            let _ = (&mut stdout).take(OUTPUT_LIMIT).read_to_end(&mut output);
            // and the rest is let go of, which is all that the pipe needs
            let _ = std::io::copy(&mut stdout, &mut std::io::sink());
            String::from_utf8_lossy(&output).into_owned()
        })
    });

    let process = HANDLE(child.as_raw_handle());

    // in a job of its own, so that what it started can be stopped with it
    let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.ok();
    if let Some(job) = job {
        let _ = unsafe { AssignProcessToJobObject(job, process) };
    }

    let in_time = unsafe {
        WaitForSingleObject(
            process,
            u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX),
        )
    } == WAIT_OBJECT_0;

    if !in_time {
        match job {
            Some(job) => {
                let _ = unsafe { TerminateJobObject(job, 1) };
            }
            None => {
                let _ = child.kill();
            }
        }
    }

    let status = child.wait();
    if let Some(job) = job {
        let _ = unsafe { CloseHandle(job) };
    }

    // once it is gone, so is the end of the pipe, and what it wrote has been read
    let output = reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();

    if !in_time {
        return Outcome::TimedOut;
    }

    match status {
        Ok(status) => Outcome::Ran {
            code: status.code().unwrap_or(-1),
            output,
        },
        Err(error) => Outcome::CouldNotRun(error.to_string()),
    }
}

/// The commands of the conditions that the whkdrc defines, by name
static DEFINITIONS: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Names that have been reported as going wrong, so that it is said once
static REPORTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// Remember the conditions that the whkdrc defines. Meant to be done once
pub fn define(checks: &[whkd_core::Check]) {
    let _ = DEFINITIONS.set(
        checks
            .iter()
            .map(|check| (check.name.clone(), check.command.clone()))
            .collect(),
    );
}

/// What is asked of a condition for as long as one key press takes: the command is run the first
/// time that it is asked about, whether it is as a true or false condition or for what it writes,
/// and what it came to is what the rest are told
pub struct PressChecks<'a> {
    definitions: &'a HashMap<String, String>,
    run: &'a dyn Fn(&str) -> Outcome,
    report: &'a dyn Fn(&str, &Outcome),
    outcomes: RefCell<HashMap<String, Outcome>>,
}

impl<'a> PressChecks<'a> {
    pub fn new(
        definitions: &'a HashMap<String, String>,
        run: &'a dyn Fn(&str) -> Outcome,
        report: &'a dyn Fn(&str, &Outcome),
    ) -> Self {
        Self {
            definitions,
            run,
            report,
            outcomes: RefCell::new(HashMap::new()),
        }
    }

    /// What running the command of `name` came to, which is only done once
    fn outcome(&self, name: &str) -> Option<Outcome> {
        if let Some(outcome) = self.outcomes.borrow().get(name) {
            return Some(outcome.clone());
        }

        // the parser does not let a name that is not defined be used
        let command = self.definitions.get(name)?;

        let outcome = (self.run)(command);
        if outcome.problem().is_some() {
            (self.report)(name, &outcome);
        }

        self.outcomes
            .borrow_mut()
            .insert(name.to_string(), outcome.clone());
        Some(outcome)
    }
}

impl Checks for PressChecks<'_> {
    fn holds(&self, name: &str) -> bool {
        self.outcome(name).is_some_and(|outcome| outcome.holds())
    }

    fn says(&self, name: &str) -> Option<String> {
        self.outcome(name).and_then(|outcome| outcome.says())
    }
}

/// Say that the command of a condition went wrong, once for each condition
pub fn report_once(name: &str, outcome: &Outcome) {
    let Some(problem) = outcome.problem() else {
        return;
    };

    let reported = REPORTED.get_or_init(|| Mutex::new(HashSet::new()));
    if reported.lock().insert(name.to_string()) {
        eprintln!("The command of `{name}` {problem}");
    }
}

/// Run `then` with the checks for a key press while `app` has focus
pub fn with_press_checks<R>(app: &str, then: impl FnOnce(&dyn Checks) -> R) -> R {
    static NONE: OnceLock<HashMap<String, String>> = OnceLock::new();
    let definitions = DEFINITIONS
        .get()
        .unwrap_or_else(|| NONE.get_or_init(HashMap::new));

    let run = |command: &str| run(command, app, TIMEOUT);
    let checks = PressChecks::new(definitions, &run, &report_once);
    then(&checks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::Instant;

    fn definitions(definitions: &[(&str, &str)]) -> HashMap<String, String> {
        definitions
            .iter()
            .map(|(name, command)| (name.to_string(), command.to_string()))
            .collect()
    }

    fn ran(code: i32, output: &str) -> Outcome {
        Outcome::Ran {
            code,
            output: output.to_string(),
        }
    }

    /// What a command exited with, if it did
    fn code(outcome: &Outcome) -> Option<i32> {
        match outcome {
            Outcome::Ran { code, .. } => Some(*code),
            _ => None,
        }
    }

    #[test]
    fn a_command_that_exits_with_0_holds() {
        assert!(run("exit 0", "x", TIMEOUT).holds());
        assert!(run("echo hello >nul", "x", TIMEOUT).holds());
    }

    #[test]
    fn a_command_that_exits_with_anything_else_does_not() {
        assert_eq!(code(&run("exit 1", "x", TIMEOUT)), Some(1));
        assert_eq!(code(&run("exit 7", "x", TIMEOUT)), Some(7));
        assert!(!run("exit 7", "x", TIMEOUT).holds());
        // a program that is not there
        assert!(!run("no-such-program-here 2>nul", "x", TIMEOUT).holds());
    }

    #[test]
    fn what_is_in_the_command_is_read_as_written() {
        // quotes, pipes and redirects, and the things that are said about them in cmd
        assert!(run("echo \"a b\" | findstr /c:\"a b\" >nul", "x", TIMEOUT).holds());
        assert_eq!(
            code(&run("echo one | findstr two >nul", "x", TIMEOUT)),
            Some(1)
        );
        assert!(run("if 1==1 (exit 0) else (exit 3)", "x", TIMEOUT).holds());
        assert!(run("exit 0 & exit 4", "x", TIMEOUT).holds());
        assert_eq!(code(&run("exit 2 && exit 0", "x", TIMEOUT)), Some(2));
    }

    #[test]
    fn what_a_command_writes_is_what_it_says() {
        assert_eq!(
            run("echo work", "x", TIMEOUT).says().as_deref(),
            Some("work")
        );
        // without the white space around it, and not in it
        assert_eq!(
            run("echo   Google Chrome   ", "x", TIMEOUT)
                .says()
                .as_deref(),
            Some("Google Chrome")
        );
        assert_eq!(
            run("(echo one)&(echo two)", "x", TIMEOUT).says().as_deref(),
            Some("one\r\ntwo")
        );
        assert_eq!(run("exit 0", "x", TIMEOUT).says().as_deref(), Some(""));
    }

    #[test]
    fn nothing_is_said_by_a_command_that_does_not_exit_with_0() {
        let outcome = run("(echo work)&exit 1", "x", TIMEOUT);
        assert_eq!(outcome, ran(1, "work\r\n"));
        assert_eq!(outcome.says(), None);
    }

    #[test]
    fn what_a_command_writes_to_the_error_output_is_not_what_it_says() {
        assert_eq!(
            run("echo work 1>&2", "x", TIMEOUT).says().as_deref(),
            Some("")
        );
    }

    #[test]
    fn a_command_that_writes_a_lot_is_not_held_up_by_it() {
        // a file of about 600 kilobytes
        let path = std::env::temp_dir().join(format!("whkd-big-{}.txt", std::process::id()));
        std::fs::write(&path, "x".repeat(600_000)).unwrap();

        let start = Instant::now();
        let outcome = run(
            &format!("type \"{}\"", path.display()),
            "x",
            Duration::from_secs(5),
        );
        std::fs::remove_file(&path).unwrap();

        assert!(outcome.holds(), "{:?}", code(&outcome));
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "{:?}",
            start.elapsed()
        );
        // and only some of it is kept
        assert_eq!(outcome.says().unwrap().len(), OUTPUT_LIMIT as usize);
    }

    #[test]
    fn the_command_is_told_which_application_has_focus() {
        let command = "if \"%WHKD_APP%\"==\"Google Chrome\" (exit 0) else (exit 1)";

        assert!(run(command, "Google Chrome", TIMEOUT).holds());
        assert!(!run(command, "Firefox", TIMEOUT).holds());
        assert!(!run(command, "", TIMEOUT).holds());

        // and can write it
        assert_eq!(
            run("echo %WHKD_APP%", "Google Chrome", TIMEOUT)
                .says()
                .as_deref(),
            Some("Google Chrome")
        );
    }

    #[test]
    fn a_command_that_takes_too_long_is_stopped() {
        let start = Instant::now();
        let outcome = run("ping -n 20 127.0.0.1 >nul", "x", Duration::from_millis(200));
        let took = start.elapsed();

        assert_eq!(outcome, Outcome::TimedOut);
        assert!(took >= Duration::from_millis(200), "{took:?}");
        assert!(took < Duration::from_secs(3), "{took:?}");
        // and nothing can be said about it
        assert!(!outcome.holds());
        assert_eq!(outcome.says(), None);
    }

    #[test]
    fn a_command_that_writes_and_then_takes_too_long_says_nothing() {
        let outcome = run(
            "(echo work)&ping -n 20 127.0.0.1 >nul",
            "x",
            Duration::from_millis(300),
        );

        assert_eq!(outcome, Outcome::TimedOut);
        assert_eq!(outcome.says(), None);
    }

    #[test]
    fn what_a_command_started_is_stopped_with_it() {
        // the ping is a program of its own that cmd waits for: if it were left running, it would go on
        // for 20 seconds, with nothing to say that it was started for this
        let marker = format!("127.0.0.{}", 100 + std::process::id() % 100);
        let command = format!("ping -n 20 {marker} >nul");

        assert_eq!(
            run(&command, "x", Duration::from_millis(300)),
            Outcome::TimedOut
        );

        std::thread::sleep(Duration::from_millis(300));
        let still_running = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "(Get-CimInstance Win32_Process -Filter \"Name='PING.EXE'\" | Where-Object {{ $_.CommandLine -like '*{marker}*' }} | Measure-Object).Count"
                ),
            ])
            .output()
            .unwrap();

        assert_eq!(
            String::from_utf8_lossy(&still_running.stdout).trim(),
            "0",
            "the ping that the command started is still running"
        );
    }

    #[test]
    fn a_command_that_is_quick_is_quick() {
        let start = Instant::now();
        assert!(run("exit 0", "x", TIMEOUT).holds());
        assert!(
            start.elapsed() < Duration::from_millis(400),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_problem_is_a_timeout_or_a_command_that_could_not_be_run() {
        assert_eq!(ran(0, "").problem(), None);
        assert_eq!(ran(1, "x").problem(), None);
        assert!(Outcome::TimedOut
            .problem()
            .unwrap()
            .contains("500 milliseconds"));
        assert!(Outcome::CouldNotRun(String::from("no such file"))
            .problem()
            .unwrap()
            .contains("no such file"));
    }

    #[test]
    fn a_condition_is_found_out_once_for_a_key_press() {
        let definitions = definitions(&[("docked", "docked.exe"), ("wide", "wide.exe")]);
        let asked = RefCell::new(vec![]);
        let run = |command: &str| {
            asked.borrow_mut().push(command.to_string());
            ran(0, "yes")
        };
        let reported = Cell::new(0);
        let report = |_: &str, _: &Outcome| reported.set(reported.get() + 1);

        let checks = PressChecks::new(&definitions, &run, &report);

        assert!(checks.holds("docked"));
        assert!(checks.holds("docked"));
        assert!(checks.holds("wide"));
        assert!(checks.holds("docked"));

        assert_eq!(*asked.borrow(), ["docked.exe", "wide.exe"]);
        assert_eq!(reported.get(), 0);
    }

    #[test]
    fn a_condition_that_is_used_both_ways_is_run_once() {
        let definitions = definitions(&[("mode", "mode.exe")]);
        let asked = Cell::new(0);
        let run = |_: &str| {
            asked.set(asked.get() + 1);
            ran(0, "work\r\n")
        };

        let checks = PressChecks::new(&definitions, &run, &|_: &str, _: &Outcome| {});

        assert!(checks.holds("mode"));
        assert_eq!(checks.says("mode").as_deref(), Some("work"));
        assert_eq!(checks.says("mode").as_deref(), Some("work"));
        assert_eq!(asked.get(), 1);
    }

    #[test]
    fn what_a_condition_said_is_forgotten_for_the_next_key_press() {
        let definitions = definitions(&[("docked", "docked.exe")]);
        let asked = Cell::new(0);
        let run = |_: &str| {
            asked.set(asked.get() + 1);
            ran(0, "")
        };

        for _ in 0..3 {
            let checks = PressChecks::new(&definitions, &run, &|_: &str, _: &Outcome| {});
            assert!(checks.holds("docked"));
        }

        assert_eq!(asked.get(), 3);
    }

    #[test]
    fn a_condition_that_does_not_hold_is_remembered_too() {
        let definitions = definitions(&[("docked", "docked.exe")]);
        let asked = Cell::new(0);
        let run = |_: &str| {
            asked.set(asked.get() + 1);
            ran(1, "")
        };

        let checks = PressChecks::new(&definitions, &run, &|_: &str, _: &Outcome| {});
        assert!(!checks.holds("docked"));
        assert!(!checks.holds("docked"));
        assert_eq!(checks.says("docked"), None);
        assert_eq!(asked.get(), 1);
    }

    #[test]
    fn a_command_that_goes_wrong_is_said_to_have_gone_wrong_and_says_nothing() {
        let definitions = definitions(&[("slow", "x"), ("broken", "y"), ("no", "z")]);
        let run = |command: &str| match command {
            "x" => Outcome::TimedOut,
            "y" => Outcome::CouldNotRun(String::from("oops")),
            _ => ran(1, ""),
        };
        let reported = RefCell::new(vec![]);
        let report = |name: &str, outcome: &Outcome| {
            reported
                .borrow_mut()
                .push((name.to_string(), outcome.clone()));
        };

        let checks = PressChecks::new(&definitions, &run, &report);

        assert!(!checks.holds("slow"));
        assert_eq!(checks.says("slow"), None);
        assert!(!checks.holds("broken"));
        assert!(!checks.holds("no"));

        // the ones that went wrong, and not the one that did not hold, each once
        assert_eq!(
            *reported.borrow(),
            [
                (String::from("slow"), Outcome::TimedOut),
                (
                    String::from("broken"),
                    Outcome::CouldNotRun(String::from("oops"))
                ),
            ]
        );
    }

    #[test]
    fn a_name_that_is_not_defined_does_not_hold_and_says_nothing() {
        let definitions = definitions(&[]);
        let run = |_: &str| panic!("there is nothing to run");

        let checks = PressChecks::new(&definitions, &run, &|_: &str, _: &Outcome| {});
        assert!(!checks.holds("docked"));
        assert_eq!(checks.says("docked"), None);
    }

    #[test]
    fn the_checks_of_a_key_press_run_real_commands() {
        define(&[
            whkd_core::Check {
                name: String::from("yes"),
                command: String::from("exit 0"),
            },
            whkd_core::Check {
                name: String::from("no"),
                command: String::from("exit 1"),
            },
            whkd_core::Check {
                name: String::from("app"),
                command: String::from("echo %WHKD_APP%"),
            },
        ]);

        with_press_checks("Firefox", |checks| {
            assert!(checks.holds("yes"));
            assert!(!checks.holds("no"));
            assert_eq!(checks.says("app").as_deref(), Some("Firefox"));
        });

        with_press_checks("Code", |checks| {
            assert_eq!(checks.says("app").as_deref(), Some("Code"));
        });
    }
}
