//! Remote control server: a Unix socket private to the user, one thread
//! accepting connections and two per connection (reader and writer). The
//! threads only move lines; commands reach the frontend through a channel
//! and are applied on its UI thread by [`Server::poll`], never on the audio
//! thread.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};

use super::{Command, MAX_LINE, error_json};

/// The socket: `$XDG_RUNTIME_DIR/voice-strip/ctl.sock` on Linux, the
/// per-user `$TMPDIR` on macOS. `None` without either, rather than falling
/// back to a folder other users can write to.
pub fn socket_path() -> Option<PathBuf> {
    ["XDG_RUNTIME_DIR", "TMPDIR"]
        .into_iter()
        .filter_map(std::env::var_os)
        .find(|d| !d.is_empty())
        .map(|d| PathBuf::from(d).join("voice-strip").join("ctl.sock"))
}

#[derive(Debug)]
pub enum StartError {
    /// Neither `XDG_RUNTIME_DIR` nor `TMPDIR` is set.
    NoRuntimeDir,
    /// Another Voice Strip answers on the socket.
    AlreadyRunning,
    /// The socket folder is not a folder of the current user.
    NotOwned(PathBuf),
    Io(io::Error),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NoRuntimeDir => f.write_str("no XDG_RUNTIME_DIR or TMPDIR"),
            StartError::AlreadyRunning => f.write_str("another Voice Strip is listening"),
            StartError::NotOwned(p) => {
                write!(f, "{} is not a folder of the current user", p.display())
            }
            StartError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for StartError {
    fn from(e: io::Error) -> Self {
        StartError::Io(e)
    }
}

/// Lines waiting for one client, about two seconds of watch lines. A
/// client that stops reading fills it and is disconnected.
const QUEUE: usize = 64;

/// The way back to one client: its queue and its socket, to hang up.
struct Client {
    out: SyncSender<String>,
    stream: UnixStream,
}

impl Client {
    /// Queues `line`; false when the client is gone or not reading, in
    /// which case it is hung up.
    fn send(&self, line: String) -> bool {
        match self.out.try_send(line) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                let _ = self.stream.shutdown(std::net::Shutdown::Both);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }
}

/// A command line received from a client, with the way back to it.
struct Incoming {
    command: Result<Command, String>,
    client: Client,
}

pub struct Server {
    path: PathBuf,
    rx: Receiver<Incoming>,
    /// Clients that sent `watch`; dropped when their connection closes.
    watchers: Vec<Client>,
    /// Last line published, so an unchanged state is not sent again.
    last: String,
}

impl Server {
    /// Listens on [`socket_path`].
    pub fn start() -> Result<Self, StartError> {
        Self::start_at(socket_path().ok_or(StartError::NoRuntimeDir)?)
    }

    pub fn start_at(path: PathBuf) -> Result<Self, StartError> {
        if let Some(dir) = path.parent() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
            check_owned(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        if UnixStream::connect(&path).is_ok() {
            return Err(StartError::AlreadyRunning);
        }
        // A socket left behind by a run that did not exit cleanly.
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("remote-accept".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let tx = tx.clone();
                    let _ = std::thread::Builder::new()
                        .name("remote-conn".into())
                        .spawn(move || serve(stream, tx));
                }
            })?;
        Ok(Self {
            path,
            rx,
            watchers: Vec::new(),
            last: String::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Answers every pending command. `handle` applies one command and
    /// returns the line to send back (normally the state as JSON). A watch
    /// request is answered like `status` and the client joins the watchers.
    pub fn poll(&mut self, mut handle: impl FnMut(Command) -> String) {
        while let Ok(inc) = self.rx.try_recv() {
            let line = match inc.command {
                Ok(Command::Watch) => {
                    if let (Ok(stream), out) =
                        (inc.client.stream.try_clone(), inc.client.out.clone())
                    {
                        self.watchers.push(Client { out, stream });
                    }
                    // The newcomer must get the next published line even if
                    // nothing changes.
                    self.last.clear();
                    handle(Command::Status)
                }
                Ok(c) => handle(c),
                Err(e) => error_json(&e),
            };
            inc.client.send(line);
        }
    }

    /// Whether any client watches; frontends compute meters only then.
    pub fn is_watched(&self) -> bool {
        !self.watchers.is_empty()
    }

    /// Sends `line` to every watcher unless it equals the last one sent.
    pub fn publish(&mut self, line: String) {
        if line == self.last || self.watchers.is_empty() {
            return;
        }
        self.watchers.retain(|w| w.send(line.clone()));
        self.last = line;
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Reads command lines from one client until it hangs up. Replies and
/// watch lines go through a writer thread, so a slow client never blocks
/// the frontend.
/// Refuses a socket folder that is a symlink or belongs to another user
/// (a shared `$TMPDIR` such as `/tmp`). The current user is the owner of a
/// file it creates next to the folder.
fn check_owned(dir: &Path) -> Result<(), StartError> {
    let meta = std::fs::symlink_metadata(dir)?;
    let parent = dir.parent().unwrap_or(Path::new("."));
    static PROBES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = PROBES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let probe = parent.join(format!(".voice-strip-probe-{}-{n}", std::process::id()));
    let me = std::fs::File::create_new(&probe).and_then(|f| f.metadata());
    let _ = std::fs::remove_file(&probe);
    if !meta.is_dir() || meta.uid() != me?.uid() {
        return Err(StartError::NotOwned(dir.to_path_buf()));
    }
    Ok(())
}

fn serve(stream: UnixStream, tx: Sender<Incoming>) {
    let (Ok(mut writer), Ok(hangup)) = (stream.try_clone(), stream.try_clone()) else {
        return;
    };
    let (out, out_rx) = mpsc::sync_channel::<String>(QUEUE);
    let client = || -> Option<Client> {
        Some(Client {
            out: out.clone(),
            stream: hangup.try_clone().ok()?,
        })
    };
    let spawned = std::thread::Builder::new()
        .name("remote-write".into())
        .spawn(move || {
            for line in out_rx {
                if writeln!(writer, "{line}").is_err() {
                    break;
                }
            }
            let _ = writer.shutdown(std::net::Shutdown::Both);
        });
    if spawned.is_err() {
        return;
    }
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::with_capacity(MAX_LINE + 1);
    loop {
        buf.clear();
        match (&mut reader)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut buf)
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.last() != Some(&b'\n') && buf.len() > MAX_LINE {
            if let Some(c) = client() {
                c.send(error_json("command too long"));
            }
            break;
        }
        let text = String::from_utf8_lossy(&buf);
        if text.trim().is_empty() {
            continue;
        }
        let Some(client) = client() else {
            break;
        };
        let incoming = Incoming {
            command: Command::parse(&text),
            client,
        };
        if tx.send(incoming).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn temp_socket(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("voice-strip-test-{}-{name}", std::process::id()));
        dir.join("ctl.sock")
    }

    /// Polls the server until `done` holds or a second passes.
    fn pump(server: &mut Server, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() && start.elapsed() < Duration::from_secs(1) {
            server.poll(|c| format!("{{\"cmd\":\"{c:?}\"}}"));
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn read_line(r: &mut BufReader<UnixStream>) -> String {
        let mut s = String::new();
        r.read_line(&mut s).unwrap();
        s.trim_end().to_string()
    }

    #[test]
    fn answers_commands_and_errors() {
        let path = temp_socket("answers");
        let mut server = Server::start_at(path.clone()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);

        let stream = UnixStream::connect(&path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut w = stream.try_clone().unwrap();
        let mut r = BufReader::new(stream);
        writeln!(w, "preset call").unwrap();
        writeln!(w, "dance").unwrap();
        let mut lines = Vec::new();
        pump(&mut server, || false);
        lines.push(read_line(&mut r));
        lines.push(read_line(&mut r));
        assert_eq!(lines[0], r#"{"cmd":"SetPreset(Call)"}"#);
        assert_eq!(lines[1], r#"{"error":"unknown command: dance"}"#);

        assert!(matches!(
            Server::start_at(path.clone()),
            Err(StartError::AlreadyRunning)
        ));
        drop(server);
        assert!(!path.exists());
    }

    #[test]
    fn watchers_get_changes_once_and_leave_on_hangup() {
        let path = temp_socket("watch");
        let mut server = Server::start_at(path.clone()).unwrap();
        let stream = UnixStream::connect(&path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut w = stream.try_clone().unwrap();
        let mut r = BufReader::new(stream);
        writeln!(w, "watch").unwrap();
        pump(&mut server, || false);
        assert!(server.is_watched());
        assert_eq!(read_line(&mut r), r#"{"cmd":"Status"}"#);

        server.publish("a".into());
        server.publish("a".into());
        server.publish("b".into());
        assert_eq!(read_line(&mut r), "a");
        assert_eq!(read_line(&mut r), "b");

        drop(r);
        drop(w);
        // The writer notices the hangup on its next write.
        let start = Instant::now();
        let mut n = 0;
        while server.is_watched() && start.elapsed() < Duration::from_secs(2) {
            n += 1;
            server.publish(format!("frame {n}"));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!server.is_watched());
    }

    #[test]
    fn refuses_a_symlinked_folder() {
        let base =
            std::env::temp_dir().join(format!("voice-strip-test-{}-link", std::process::id()));
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.join("voice-strip");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let r = Server::start_at(link.join("ctl.sock"));
        assert!(matches!(r, Err(StartError::NotOwned(_))));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_client_that_stops_reading_is_dropped() {
        let path = temp_socket("stuck");
        let mut server = Server::start_at(path.clone()).unwrap();
        let stream = UnixStream::connect(&path).unwrap();
        let mut w = stream.try_clone().unwrap();
        writeln!(w, "watch").unwrap();
        pump(&mut server, || false);
        // Never read: the kernel buffer and then the queue fill up.
        let big = "x".repeat(4096);
        let start = Instant::now();
        let mut n = 0;
        while server.is_watched() && start.elapsed() < Duration::from_secs(5) {
            n += 1;
            server.publish(format!("{n} {big}"));
        }
        assert!(!server.is_watched(), "stuck client still watched");
        drop(stream);
    }

    #[test]
    fn long_lines_are_refused() {
        let path = temp_socket("long");
        let mut server = Server::start_at(path.clone()).unwrap();
        let stream = UnixStream::connect(&path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut w = stream.try_clone().unwrap();
        let mut r = BufReader::new(stream);
        writeln!(w, "{}", "x".repeat(MAX_LINE * 2)).unwrap();
        pump(&mut server, || false);
        assert_eq!(read_line(&mut r), r#"{"error":"command too long"}"#);
    }
}
