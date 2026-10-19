//! `voice-strip ctl <command>`: sends one command to the running app and
//! prints its answer. Shared by every frontend binary.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use super::Command;
use super::server::socket_path;

/// What a client prints when no Voice Strip answers.
pub const NOT_RUNNING: &str = r#"{"running":false}"#;

const USAGE: &str = "usage: voice-strip ctl <command>

  status                      state as JSON
  watch                       state as JSON lines, on every change
  on | off | toggle           processing
  mute on | off | toggle      others hear silence
  preset <id>                 broadcast podcast call stream narration
  noise <level>               off light medium strong (needs the model)
  intensity <0-100>
  phones on | off | toggle    headphone monitor
  model download              download the noise removal model (34.7 MB)
  show                        open the window (starts the app if closed)
  hide                        hide the window; the app keeps running
  start                       start the app without its window if closed
  quit

Exit status: 0 on success, 1 when the app is not running or refuses the
command, 2 on a usage error.";

fn connect() -> io::Result<UnixStream> {
    let path = socket_path().ok_or_else(|| io::Error::other("no XDG_RUNTIME_DIR or TMPDIR"))?;
    UnixStream::connect(path)
}

/// Sends one command line and returns the first line of the answer.
pub fn request(line: &str) -> io::Result<String> {
    let stream = connect()?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    (&stream).write_all(format!("{}\n", line.trim()).as_bytes())?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    if answer.is_empty() {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    Ok(answer.trim_end().to_string())
}

/// Runs the `ctl` subcommand with the arguments after `ctl`; returns the
/// process exit status.
pub fn cli(args: &[String]) -> i32 {
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        eprintln!("{USAGE}");
        return if args.is_empty() { 2 } else { 0 };
    }
    let line = args.join(" ");
    let command = match Command::parse(&line) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("voice-strip ctl: {e}");
            return 2;
        }
    };
    if command == Command::Watch {
        return watch(&line);
    }
    match request(&line) {
        Ok(answer) => {
            println!("{answer}");
            i32::from(answer.starts_with("{\"error\""))
        }
        Err(_) if command == Command::Show => launch(&[]),
        Err(_) if command == Command::Start => launch(&["--background"]),
        Err(_) => {
            println!("{NOT_RUNNING}");
            1
        }
    }
}

/// Prints every line the app sends until it closes the connection.
fn watch(line: &str) -> i32 {
    let Ok(stream) = connect() else {
        println!("{NOT_RUNNING}");
        return 1;
    };
    if (&stream)
        .write_all(format!("{}\n", line.trim()).as_bytes())
        .is_err()
    {
        println!("{NOT_RUNNING}");
        return 1;
    }
    let stdout = io::stdout();
    for l in BufReader::new(stream).lines() {
        let Ok(l) = l else { break };
        let mut out = stdout.lock();
        if writeln!(out, "{l}").and_then(|_| out.flush()).is_err() {
            return 0;
        }
    }
    println!("{NOT_RUNNING}");
    0
}

/// Starts the app, detached, when `show` or `start` finds it closed.
fn launch(args: &[&str]) -> i32 {
    let started = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    });
    match started {
        Ok(_) => {
            println!(r#"{{"running":false,"starting":true}}"#);
            0
        }
        Err(e) => {
            eprintln!("voice-strip ctl: cannot start Voice Strip: {e}");
            1
        }
    }
}
