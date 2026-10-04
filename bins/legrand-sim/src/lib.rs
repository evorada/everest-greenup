//! A fake Legrand power board behind a TCP socket.
//!
//! Two listeners: the *board* port speaks the serial protocol (lines
//! ending in `\r`, exactly what the real board sends over USB), the
//! *control* port takes one command per line to move the simulated
//! vehicle: `plug`, `unplug`, `draw`, `stop`, `full`, `state`.
//!
//! `socat pty,link=/tmp/legrand1,raw tcp:host:5101` turns the board port
//! into a pty when a program insists on a serial device; `greenup-runtime`
//! connects to `tcp:host:port` directly.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use board_model::{Board, Profile};

/// A running simulator.
pub struct Simulator {
    board_addr: SocketAddr,
    control_addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

/// A scripted vehicle: `(seconds after start, command)`.
pub type Script = Vec<(f64, String)>;

enum Input {
    Board(String),
    Control(String, Sender<String>),
    BoardClient(TcpStream),
}

impl Simulator {
    /// Bind both listeners (`0` picks a free port) and start serving.
    pub fn start(
        board: SocketAddr,
        control: SocketAddr,
        profile: Profile,
        script: Script,
    ) -> std::io::Result<Self> {
        let board_listener = TcpListener::bind(board)?;
        let control_listener = TcpListener::bind(control)?;
        let board_addr = board_listener.local_addr()?;
        let control_addr = control_listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        spawn_board_acceptor(board_listener, tx.clone(), stop.clone());
        spawn_control_acceptor(control_listener, tx, stop.clone());
        let stop_for_loop = stop.clone();
        let thread = thread::Builder::new()
            .name("legrand-sim".into())
            .spawn(move || serve(profile, script, rx, stop_for_loop))?;
        Ok(Simulator {
            board_addr,
            control_addr,
            stop,
            thread: Some(thread),
        })
    }

    /// Where the board protocol is served.
    pub fn board_addr(&self) -> SocketAddr {
        self.board_addr
    }

    /// Where vehicle commands are taken.
    pub fn control_addr(&self) -> SocketAddr {
        self.control_addr
    }

    /// Send one control command and return the simulator's answer.
    pub fn control(&self, command: &str) -> std::io::Result<String> {
        let mut stream = TcpStream::connect_timeout(&self.control_addr, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.write_all(format!("{command}\n").as_bytes())?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(line.trim().to_owned())
    }

    /// Stop serving.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock the acceptors.
        let _ = TcpStream::connect_timeout(&self.board_addr, Duration::from_millis(200));
        let _ = TcpStream::connect_timeout(&self.control_addr, Duration::from_millis(200));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn spawn_board_acceptor(listener: TcpListener, tx: Sender<Input>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        for stream in listener.incoming() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            if tx.send(Input::BoardClient(stream)).is_err() {
                break;
            }
        }
    });
}

fn spawn_control_acceptor(listener: TcpListener, tx: Sender<Input>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        for stream in listener.incoming() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let tx = tx.clone();
            thread::spawn(move || {
                let Ok(writer) = stream.try_clone() else {
                    return;
                };
                let mut writer = writer;
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    let (reply_tx, reply_rx) = mpsc::channel();
                    if tx.send(Input::Control(line, reply_tx)).is_err() {
                        break;
                    }
                    let reply = reply_rx.recv().unwrap_or_else(|_| "error".to_owned());
                    if writeln!(writer, "{reply}").is_err() {
                        break;
                    }
                }
            });
        }
    });
}

/// Reads the client's lines (terminated by `\r` or `\n`) into the queue.
fn spawn_board_reader(stream: TcpStream, tx: Sender<Input>) {
    thread::spawn(move || {
        let mut buf = [0u8; 256];
        let mut pending = Vec::new();
        let mut stream = stream;
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &b in &buf[..n] {
                        if b == b'\r' || b == b'\n' {
                            if !pending.is_empty() {
                                let line = String::from_utf8_lossy(&pending).into_owned();
                                pending.clear();
                                if tx.send(Input::Board(line)).is_err() {
                                    return;
                                }
                            }
                        } else if pending.len() < 256 {
                            pending.push(b);
                        }
                    }
                }
            }
        }
    });
}

fn apply_control(board: &mut Board, command: &str, now: Instant) -> (Vec<String>, String) {
    let lines = match command.trim() {
        "plug" => board.plug(now),
        "unplug" => board.unplug(now),
        "draw" => board.ev_draw(true, now),
        "stop" => board.ev_draw(false, now),
        "full" => board.ev_full(now),
        "state" => Vec::new(),
        other => return (Vec::new(), format!("unknown command {other:?}")),
    };
    (
        lines,
        format!(
            "ok state={} cc={} t2={} fm={}",
            board.state(),
            board.cc(),
            u8::from(board.t2_allowed()),
            board.fm()
        ),
    )
}

fn serve(profile: Profile, mut script: Script, rx: Receiver<Input>, stop: Arc<AtomicBool>) {
    let start = Instant::now();
    let mut board = Board::new(profile, start);
    let mut client: Option<TcpStream> = None;
    let (reader_tx, reader_rx) = mpsc::channel::<Input>();
    script.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut script = script.into_iter().peekable();
    while !stop.load(Ordering::SeqCst) {
        let now = Instant::now();
        let mut out = Vec::new();
        // New connections and control commands.
        while let Ok(input) = rx.try_recv() {
            match input {
                Input::BoardClient(stream) => {
                    let _ = stream.set_nodelay(true);
                    if let Ok(reader) = stream.try_clone() {
                        spawn_board_reader(reader, reader_tx.clone());
                        client = Some(stream);
                    }
                }
                Input::Control(command, reply) => {
                    let (lines, answer) = apply_control(&mut board, &command, now);
                    out.extend(lines);
                    let _ = reply.send(answer);
                }
                Input::Board(line) => out.extend(board.input_line(&line, now)),
            }
        }
        while let Ok(Input::Board(line)) = reader_rx.try_recv() {
            out.extend(board.input_line(&line, now));
        }
        while let Some((at, _)) = script.peek() {
            if now.duration_since(start).as_secs_f64() < *at {
                break;
            }
            if let Some((_, command)) = script.next() {
                let (lines, _) = apply_control(&mut board, &command, now);
                out.extend(lines);
            }
        }
        out.extend(board.tick(now));
        if let Some(stream) = client.as_mut() {
            for line in &out {
                if stream.write_all(format!("{line}\r").as_bytes()).is_err() {
                    client = None;
                    break;
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}
