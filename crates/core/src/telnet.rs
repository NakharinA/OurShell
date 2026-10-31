//! Telnet client for interactive terminal sessions (RFC 854 and friends).
//!
//! Telnet has no encryption and no authentication protocol: the server just
//! prints a login prompt and the user types into it. It exists here for
//! network gear, serial-console servers and legacy boxes that don't speak SSH.
//!
//! Negotiated options: ECHO and SUPPRESS-GO-AHEAD from the server, NAWS
//! (window size) and TERMINAL-TYPE from us, BINARY in both directions.
//! Everything else is refused. Sessions surface the same [`PtyEvent`]s as an
//! SSH PTY so the agent can treat both alike.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use crate::ssh::PtyEvent;

pub const DEFAULT_PORT: u16 = 23;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

const OPT_BINARY: u8 = 0;
const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;
const OPT_TTYPE: u8 = 24;
const OPT_NAWS: u8 = 31;

const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum TelnetError {
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("connection to {0} timed out")]
    Timeout(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, TelnetError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParseState {
    Data,
    Iac,
    Verb(u8),
    Sb,
    SbIac,
}

/// Telnet option negotiation and stream parsing, with no I/O.
#[derive(Debug)]
pub struct Negotiator {
    term: String,
    cols: u16,
    rows: u16,
    /// Options we perform (answered `WILL`).
    local: [bool; 256],
    /// Options the server performs (answered `DO`).
    remote: [bool; 256],
    state: ParseState,
    sb: Vec<u8>,
    /// The previous data byte was a CR (a following NUL is padding).
    after_cr: bool,
}

impl Negotiator {
    pub fn new(term: &str, cols: u16, rows: u16) -> Self {
        Self {
            term: term.to_string(),
            cols,
            rows,
            local: [false; 256],
            remote: [false; 256],
            state: ParseState::Data,
            sb: Vec::new(),
            after_cr: false,
        }
    }

    /// What we send right after connecting: offer window size and terminal
    /// type, and ask the server to stop sending go-aheads.
    pub fn greeting(&mut self) -> Vec<u8> {
        self.local[OPT_NAWS as usize] = true;
        self.local[OPT_TTYPE as usize] = true;
        self.remote[OPT_SGA as usize] = true;
        vec![IAC, WILL, OPT_NAWS, IAC, WILL, OPT_TTYPE, IAC, DO, OPT_SGA]
    }

    fn local_supported(opt: u8) -> bool {
        matches!(opt, OPT_BINARY | OPT_SGA | OPT_TTYPE | OPT_NAWS)
    }

    fn remote_supported(opt: u8) -> bool {
        matches!(opt, OPT_BINARY | OPT_ECHO | OPT_SGA)
    }

    /// Parses bytes from the server: terminal data goes to `data`, negotiation
    /// replies to `reply`.
    pub fn feed(&mut self, input: &[u8], data: &mut Vec<u8>, reply: &mut Vec<u8>) {
        for &b in input {
            self.state = match self.state {
                ParseState::Data => {
                    if b == IAC {
                        ParseState::Iac
                    } else {
                        let pad = self.after_cr && b == 0;
                        self.after_cr = b == b'\r';
                        if !pad {
                            data.push(b);
                        }
                        ParseState::Data
                    }
                }
                ParseState::Iac => match b {
                    IAC => {
                        self.after_cr = false;
                        data.push(IAC);
                        ParseState::Data
                    }
                    DO | DONT | WILL | WONT => ParseState::Verb(b),
                    SB => {
                        self.sb.clear();
                        ParseState::Sb
                    }
                    // NOP, GA, AYT, ... carry no data for a terminal.
                    _ => ParseState::Data,
                },
                ParseState::Verb(verb) => {
                    self.negotiate(verb, b, reply);
                    ParseState::Data
                }
                ParseState::Sb => {
                    if b == IAC {
                        ParseState::SbIac
                    } else {
                        // Bound a malicious never-ending subnegotiation.
                        if self.sb.len() < 1024 {
                            self.sb.push(b);
                        }
                        ParseState::Sb
                    }
                }
                ParseState::SbIac => match b {
                    SE => {
                        self.subnegotiation(reply);
                        ParseState::Data
                    }
                    IAC => {
                        if self.sb.len() < 1024 {
                            self.sb.push(IAC);
                        }
                        ParseState::Sb
                    }
                    _ => ParseState::Sb,
                },
            };
        }
    }

    fn negotiate(&mut self, verb: u8, opt: u8, reply: &mut Vec<u8>) {
        let i = opt as usize;
        match verb {
            DO => {
                if Self::local_supported(opt) {
                    if !self.local[i] {
                        self.local[i] = true;
                        reply.extend_from_slice(&[IAC, WILL, opt]);
                    }
                    if opt == OPT_NAWS {
                        reply.extend_from_slice(&self.naws());
                    }
                } else {
                    reply.extend_from_slice(&[IAC, WONT, opt]);
                }
            }
            DONT if self.local[i] => {
                self.local[i] = false;
                reply.extend_from_slice(&[IAC, WONT, opt]);
            }
            WILL => {
                if Self::remote_supported(opt) {
                    if !self.remote[i] {
                        self.remote[i] = true;
                        reply.extend_from_slice(&[IAC, DO, opt]);
                    }
                } else {
                    reply.extend_from_slice(&[IAC, DONT, opt]);
                }
            }
            WONT if self.remote[i] => {
                self.remote[i] = false;
                reply.extend_from_slice(&[IAC, DONT, opt]);
            }
            _ => {}
        }
    }

    fn subnegotiation(&mut self, reply: &mut Vec<u8>) {
        if self.sb.first() == Some(&OPT_TTYPE)
            && self.sb.get(1) == Some(&TTYPE_SEND)
            && self.local[OPT_TTYPE as usize]
        {
            reply.extend_from_slice(&[IAC, SB, OPT_TTYPE, TTYPE_IS]);
            reply.extend(self.term.bytes().filter(|&b| b != IAC));
            reply.extend_from_slice(&[IAC, SE]);
        }
    }

    /// The `IAC SB NAWS` window-size report.
    fn naws(&self) -> Vec<u8> {
        let mut out = vec![IAC, SB, OPT_NAWS];
        for b in self
            .cols
            .to_be_bytes()
            .into_iter()
            .chain(self.rows.to_be_bytes())
        {
            out.push(b);
            if b == IAC {
                out.push(IAC);
            }
        }
        out.extend_from_slice(&[IAC, SE]);
        out
    }

    /// Records a new window size; returns the report to send, if the server
    /// accepted NAWS.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Option<Vec<u8>> {
        self.cols = cols;
        self.rows = rows;
        self.local[OPT_NAWS as usize].then(|| self.naws())
    }

    /// Encodes terminal input for the wire: doubles IAC, and outside binary
    /// mode sends a lone CR as the telnet newline CR LF (as PuTTY does).
    pub fn encode(&self, input: &[u8]) -> Vec<u8> {
        let binary = self.local[OPT_BINARY as usize];
        let mut out = Vec::with_capacity(input.len() + 4);
        for (i, &b) in input.iter().enumerate() {
            out.push(b);
            if b == IAC {
                out.push(IAC);
            } else if b == b'\r' && !binary && input.get(i + 1) != Some(&b'\n') {
                out.push(b'\n');
            }
        }
        out
    }
}

/// A live telnet session. Read [`TelnetSession::events`]; write through
/// [`TelnetSession::writer`].
pub struct TelnetSession {
    writer: TelnetWriter,
    pub events: mpsc::Receiver<PtyEvent>,
    reader_task: JoinHandle<()>,
}

impl TelnetSession {
    pub fn writer(&self) -> TelnetWriter {
        self.writer.clone()
    }
}

impl Drop for TelnetSession {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

/// A cheap, cloneable handle to the write side of a [`TelnetSession`].
#[derive(Clone)]
pub struct TelnetWriter {
    stream: Arc<Mutex<OwnedWriteHalf>>,
    neg: Arc<StdMutex<Negotiator>>,
}

impl TelnetWriter {
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        let bytes = self.neg.lock().unwrap().encode(data);
        self.stream.lock().await.write_all(&bytes).await?;
        Ok(())
    }

    pub async fn resize(&self, cols: u32, rows: u32) -> Result<()> {
        let clamp = |n: u32| u16::try_from(n).unwrap_or(u16::MAX);
        let report = self.neg.lock().unwrap().resize(clamp(cols), clamp(rows));
        if let Some(bytes) = report {
            self.stream.lock().await.write_all(&bytes).await?;
        }
        Ok(())
    }

    pub async fn close(&self) -> Result<()> {
        self.stream.lock().await.shutdown().await?;
        Ok(())
    }
}

/// Opens a telnet connection to `host:port`.
pub async fn connect(
    host: &str,
    port: u16,
    term: &str,
    cols: u32,
    rows: u32,
) -> Result<TelnetSession> {
    let addr = crate::ssh::host_port(host, port);
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| TelnetError::Timeout(addr.clone()))?
        .map_err(|e| TelnetError::Connect(format!("{addr}: {e}")))?;
    let _ = stream.set_nodelay(true);
    let (mut read, write) = stream.into_split();

    let clamp = |n: u32| u16::try_from(n).unwrap_or(u16::MAX);
    let mut neg = Negotiator::new(term, clamp(cols), clamp(rows));
    let greeting = neg.greeting();
    let neg = Arc::new(StdMutex::new(neg));
    let stream = Arc::new(Mutex::new(write));
    stream.lock().await.write_all(&greeting).await?;

    let (tx, rx) = mpsc::channel(256);
    let reader_neg = neg.clone();
    let reader_stream = stream.clone();
    let reader_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = match read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut data = Vec::with_capacity(n);
            let mut reply = Vec::new();
            reader_neg
                .lock()
                .unwrap()
                .feed(&buf[..n], &mut data, &mut reply);
            if !reply.is_empty() && reader_stream.lock().await.write_all(&reply).await.is_err() {
                break;
            }
            if !data.is_empty() && tx.send(PtyEvent::Data(data)).await.is_err() {
                return;
            }
        }
        let _ = tx.send(PtyEvent::Closed).await;
    });

    Ok(TelnetSession {
        writer: TelnetWriter { stream, neg },
        events: rx,
        reader_task,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(n: &mut Negotiator, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let (mut data, mut reply) = (Vec::new(), Vec::new());
        n.feed(input, &mut data, &mut reply);
        (data, reply)
    }

    #[test]
    fn plain_data_passes_through() {
        let mut n = Negotiator::new("xterm", 80, 24);
        let (data, reply) = feed(&mut n, b"login: ");
        assert_eq!(data, b"login: ");
        assert!(reply.is_empty());
    }

    #[test]
    fn escaped_iac_and_cr_nul() {
        let mut n = Negotiator::new("xterm", 80, 24);
        let (data, _) = feed(&mut n, &[b'a', IAC, IAC, b'\r', 0, b'b']);
        assert_eq!(data, vec![b'a', IAC, b'\r', b'b']);
    }

    #[test]
    fn accepts_server_echo_and_refuses_unknown() {
        let mut n = Negotiator::new("xterm", 80, 24);
        let (_, reply) = feed(&mut n, &[IAC, WILL, OPT_ECHO, IAC, DO, 39]);
        assert_eq!(reply, vec![IAC, DO, OPT_ECHO, IAC, WONT, 39]);
        // Repeating an accepted option doesn't re-acknowledge (no loops).
        let (_, reply) = feed(&mut n, &[IAC, WILL, OPT_ECHO]);
        assert!(reply.is_empty());
    }

    #[test]
    fn naws_sent_on_do_and_resize() {
        let mut n = Negotiator::new("xterm", 80, 24);
        let _ = n.greeting();
        let (_, reply) = feed(&mut n, &[IAC, DO, OPT_NAWS]);
        assert_eq!(reply, vec![IAC, SB, OPT_NAWS, 0, 80, 0, 24, IAC, SE]);
        assert_eq!(
            n.resize(255, 50),
            Some(vec![IAC, SB, OPT_NAWS, 0, 255, 255, 0, 50, IAC, SE])
        );
    }

    #[test]
    fn terminal_type_reply() {
        let mut n = Negotiator::new("xterm-256color", 80, 24);
        let _ = n.greeting();
        let (_, reply) = feed(&mut n, &[IAC, SB, OPT_TTYPE, TTYPE_SEND, IAC, SE]);
        let mut want = vec![IAC, SB, OPT_TTYPE, TTYPE_IS];
        want.extend_from_slice(b"xterm-256color");
        want.extend_from_slice(&[IAC, SE]);
        assert_eq!(reply, want);
    }

    #[test]
    fn negotiation_split_across_reads() {
        let mut n = Negotiator::new("xterm", 80, 24);
        let (d1, r1) = feed(&mut n, &[b'x', IAC]);
        let (d2, r2) = feed(&mut n, &[WILL]);
        let (d3, r3) = feed(&mut n, &[OPT_SGA, b'y']);
        assert_eq!([d1, d2, d3].concat(), b"xy");
        assert_eq!([r1, r2, r3].concat(), vec![IAC, DO, OPT_SGA]);
    }

    #[test]
    fn encode_escapes_iac_and_newline() {
        let n = Negotiator::new("xterm", 80, 24);
        assert_eq!(
            n.encode(&[b'a', IAC, b'\r']),
            vec![b'a', IAC, IAC, b'\r', b'\n']
        );
        assert_eq!(n.encode(b"\r\n"), b"\r\n");
    }
}
