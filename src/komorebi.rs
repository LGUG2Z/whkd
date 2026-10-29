//! A minimal client for komorebi's socket, enough to ask for the layout of the focused workspace.
//! This avoids depending on `komorebi-client`, which pulls in the whole of komorebi.

use std::io::Read;
use std::io::Write;
use std::net::Shutdown;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use uds_windows::UnixStream;
use whkd_core::Layout;

const TIMEOUT: Duration = Duration::from_millis(500);

/// komorebi expects a JSON-serialized `SocketMessage::Query(StateQuery::FocusedWorkspaceLayout)`
const QUERY_FOCUSED_WORKSPACE_LAYOUT: &str =
    r#"{"type":"Query","content":"FocusedWorkspaceLayout"}"#;

static WARNED: AtomicBool = AtomicBool::new(false);

fn socket_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("komorebi").join("komorebi.sock"))
}

/// The layout of komorebi's focused workspace, or `None` if komorebi is not running, does not
/// answer in time, or is using a layout that whkd does not know about (such as a custom layout)
pub fn focused_workspace_layout() -> Option<Layout> {
    match query(QUERY_FOCUSED_WORKSPACE_LAYOUT) {
        Ok(reply) => Layout::parse(reply.trim().trim_matches('"')),
        Err(error) => {
            // this runs on every key press, so only complain once
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "Unable to query komorebi ({error}), 'layout' conditions will not match until it is reachable"
                );
            }

            None
        }
    }
}

fn query(message: &str) -> std::io::Result<String> {
    let path = socket_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no local data directory found",
        )
    })?;

    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;

    stream.write_all(message.as_bytes())?;
    stream.shutdown(Shutdown::Write)?;

    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a running komorebi, so run with `cargo test -- --ignored`
    #[test]
    #[ignore = "requires a running komorebi"]
    fn queries_the_focused_workspace_layout() {
        let layout = focused_workspace_layout();
        println!("focused workspace layout: {layout:?}");
        assert!(layout.is_some());
    }
}
