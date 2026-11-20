//! M7: jobs, channels, timers and a minimal terminal buffer.
//!
//! RustVim keeps the single-threaded editor model: background threads only read
//! pipes/sockets and post byte chunks over `mpsc`; [pump] drains them on the
//! main thread and runs VimL callbacks there.  No editor state crosses a
//! thread boundary.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rvim_core::{VimError, VimResult};
use rvim_editor::Editor;
use rvim_viml::Env;
use rvim_viml::value::{Value, VimDict};

use crate::script::ScriptState;

/// Callbacks attached to a job.
#[derive(Clone, Default)]
pub struct Callbacks {
    out: Option<Value>,
    err: Option<Value>,
    exit: Option<Value>,
    close: Option<Value>,
}

struct Job {
    id: i64,
    channel: i64,
    program: String,
    args: Vec<String>,
    child: Arc<Mutex<Child>>,
    stdin: Option<Arc<Mutex<ChildStdin>>>,
    out_rx: Receiver<Vec<u8>>,
    err_rx: Receiver<Vec<u8>>,
    status: &'static str,
    exitval: i64,
    callbacks: Callbacks,
    out_mode: String,
    terminal: Option<i32>,
    exit_fired: bool,
}

struct Channel {
    id: i64,
    job: Option<i64>,
    stream: Option<Arc<Mutex<TcpStream>>>,
    rx: Option<Receiver<Vec<u8>>>,
    buffer: Vec<u8>,
    open: bool,
    next_msg_id: i64,
    request_id: i64,
}

struct Timer {
    id: i64,
    due: Instant,
    interval: Option<Duration>,
    callback: Value,
    paused: bool,
    fired: i64,
}

struct Terminal {
    job: i64,
    lines: Vec<String>,
    rows: i64,
    cols: i64,
}

/// All M7 runtime state.  It lives in [ScriptState] so it shares the editor's
/// lifetime and the script callbacks can be resolved from the same state.
#[derive(Default)]
pub struct M7State {
    jobs: HashMap<i64, Job>,
    channels: HashMap<i64, Channel>,
    timers: HashMap<i64, Timer>,
    terminals: HashMap<i32, Terminal>,
    next_job: i64,
    next_channel: i64,
    next_timer: i64,
    pumping: bool,
}

impl std::fmt::Debug for M7State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("M7State")
            .field("jobs", &self.jobs.len())
            .field("channels", &self.channels.len())
            .field("timers", &self.timers.len())
            .field("terminals", &self.terminals.len())
            .finish()
    }
}

impl M7State {
    fn alloc_job(&mut self) -> i64 {
        self.next_job += 1;
        self.next_job
    }

    fn alloc_channel(&mut self) -> i64 {
        self.next_channel += 1;
        self.next_channel
    }

    fn alloc_timer(&mut self) -> i64 {
        self.next_timer += 1;
        self.next_timer
    }
}

fn err(code: &'static str, message: impl Into<String>) -> VimError {
    VimError::new(code, message)
}

fn dict_get(value: &Value, key: &str) -> Option<Value> {
    let Value::Dict(dict) = value else {
        return None;
    };
    dict.borrow().get(key).cloned()
}

fn sarg(args: &[Value], index: usize) -> String {
    args.get(index)
        .map(Value::to_string_value)
        .unwrap_or_default()
}

fn narg(args: &[Value], index: usize) -> i64 {
    args.get(index).map(Value::to_number).unwrap_or(0)
}

fn spawn_reader<R: Read + Send + 'static>(mut reader: R) -> Receiver<Vec<u8>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

fn command_parts(value: &Value) -> (String, Vec<String>) {
    match value {
        Value::List(items) => {
            let parts: Vec<String> = items.borrow().iter().map(Value::to_string_value).collect();
            let program = parts.first().cloned().unwrap_or_default();
            (program, parts.into_iter().skip(1).collect())
        }
        other => {
            let command = other.to_string_value();
            if cfg!(windows) {
                ("cmd".to_string(), vec!["/C".to_string(), command])
            } else {
                ("sh".to_string(), vec!["-c".to_string(), command])
            }
        }
    }
}

/// Start a job; returns its id (0 on failure).
fn job_start(ed: &mut Editor, state: &mut ScriptState, args: &[Value]) -> VimResult<Value> {
    let (program, cmd_args) = command_parts(args.first().unwrap_or(&Value::Null));
    if program.is_empty() {
        return Ok(Value::Number(0));
    }
    let mut command = Command::new(&program);
    command.args(&cmd_args);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut callbacks = Callbacks::default();
    let mut out_mode = "nl".to_string();
    if let Some(options) = args.get(1).filter(|value| matches!(value, Value::Dict(_))) {
        if let Some(cwd) = dict_get(options, "cwd") {
            let cwd = cwd.to_string_value();
            if !cwd.is_empty() {
                command.current_dir(cwd);
            }
        }
        if let Value::Dict(env) = options.clone() {
            for (key, value) in env.borrow().iter() {
                command.env(key, value.to_string_value());
            }
        }
        callbacks.out = dict_get(options, "out_cb");
        callbacks.err = dict_get(options, "err_cb");
        callbacks.exit = dict_get(options, "exit_cb");
        callbacks.close = dict_get(options, "close_cb");
        if let Some(mode) = dict_get(options, "out_mode") {
            out_mode = mode.to_string_value();
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            ed.messages
                .info(format!("E903: process failed to start: {e}"));
            return Ok(Value::Number(0));
        }
    };
    let stdin = child.stdin.take().map(|stdin| Arc::new(Mutex::new(stdin)));
    let out_rx = child.stdout.take().map(spawn_reader).unwrap_or_else(|| {
        let (_tx, rx) = channel();
        rx
    });
    let err_rx = child.stderr.take().map(spawn_reader).unwrap_or_else(|| {
        let (_tx, rx) = channel();
        rx
    });
    let job_id = state.m7.alloc_job();
    let channel_id = state.m7.alloc_channel();
    state.m7.channels.insert(
        channel_id,
        Channel {
            id: channel_id,
            job: Some(job_id),
            stream: None,
            rx: None,
            buffer: Vec::new(),
            open: true,
            next_msg_id: 0,
            request_id: 0,
        },
    );
    state.m7.jobs.insert(
        job_id,
        Job {
            id: job_id,
            channel: channel_id,
            program,
            args: cmd_args,
            child: Arc::new(Mutex::new(child)),
            stdin,
            out_rx,
            err_rx,
            status: "run",
            exitval: 0,
            callbacks,
            out_mode,
            terminal: None,
            exit_fired: false,
        },
    );
    Ok(Value::Number(job_id))
}

fn job_status(state: &mut ScriptState, id: i64) -> &'static str {
    let Some(job) = state.m7.jobs.get_mut(&id) else {
        return "fail";
    };
    if job.status == "run" {
        if let Ok(mut child) = job.child.lock() {
            if let Ok(Some(status)) = child.try_wait() {
                job.status = "dead";
                job.exitval = status.code().map(i64::from).unwrap_or(-1);
            }
        }
    }
    job.status
}

fn drain_job(job: &mut Job, out_events: &mut Vec<String>, err_events: &mut Vec<String>) {
    while let Ok(chunk) = job.out_rx.try_recv() {
        let text = String::from_utf8_lossy(&chunk).into_owned();
        if job.out_mode == "raw" {
            out_events.push(text);
        } else {
            for line in text.split_inclusive('\n') {
                out_events.push(line.trim_end_matches(['\n', '\r']).to_string());
            }
        }
    }
    while let Ok(chunk) = job.err_rx.try_recv() {
        let text = String::from_utf8_lossy(&chunk).into_owned();
        if job.out_mode == "raw" {
            err_events.push(text);
        } else {
            for line in text.split_inclusive('\n') {
                err_events.push(line.trim_end_matches(['\n', '\r']).to_string());
            }
        }
    }
}

/// Run callbacks for pending job/timer/channel events.  Safe to call between
/// commands; re-entrant calls are ignored.
pub fn pump(ed: &mut Editor, env: &mut Env, state: &mut ScriptState) {
    if state.m7.pumping {
        return;
    }
    state.m7.pumping = true;
    let now = Instant::now();
    let mut callbacks: Vec<(Value, Vec<Value>)> = Vec::new();
    let mut terminal_lines: Vec<(i32, Vec<String>)> = Vec::new();
    let job_ids: Vec<i64> = state.m7.jobs.keys().copied().collect();
    for id in job_ids {
        let (channel, status, exitval, exit_cb, close_cb, terminal, lines) = {
            let Some(job) = state.m7.jobs.get_mut(&id) else {
                continue;
            };
            let mut outs = Vec::new();
            let mut errs = Vec::new();
            drain_job(job, &mut outs, &mut errs);
            let terminal_text = outs.clone();
            for message in outs {
                if let Some(cb) = &job.callbacks.out {
                    callbacks.push((
                        cb.clone(),
                        vec![Value::Number(id), Value::string(message.clone())],
                    ));
                }
                if let Some(channel) = state.m7.channels.get_mut(&job.channel) {
                    channel.buffer.extend_from_slice(message.as_bytes());
                    channel.buffer.push(b'\n');
                }
            }
            for message in errs {
                if let Some(cb) = &job.callbacks.err {
                    callbacks.push((
                        cb.clone(),
                        vec![Value::Number(id), Value::string(message.clone())],
                    ));
                }
            }
            if job.status == "run" {
                if let Ok(mut child) = job.child.lock() {
                    if let Ok(Some(status)) = child.try_wait() {
                        job.status = "dead";
                        job.exitval = status.code().map(i64::from).unwrap_or(-1);
                    }
                }
            }
            let lines = terminal_text;
            (
                job.channel,
                job.status,
                job.exitval,
                job.callbacks.exit.clone(),
                job.callbacks.close.clone(),
                job.terminal,
                lines,
            )
        };
        let _ = status;
        if status == "dead" {
            if let Some(cb) = exit_cb {
                if let Some(job) = state.m7.jobs.get_mut(&id) {
                    if !job.exit_fired {
                        job.exit_fired = true;
                        callbacks.push((cb, vec![Value::Number(id), Value::Number(exitval)]));
                    }
                }
            }
            if let Some(cb) = close_cb {
                callbacks.push((cb, vec![Value::Number(channel)]));
            }
        }
        if let Some(buf) = terminal {
            if !lines.is_empty() {
                terminal_lines.push((buf, lines));
            }
        }
    }
    let timer_ids: Vec<i64> = state.m7.timers.keys().copied().collect();
    for id in timer_ids {
        let mut remove = false;
        {
            let Some(timer) = state.m7.timers.get_mut(&id) else {
                continue;
            };
            if timer.paused || timer.due > now {
                continue;
            }
            callbacks.push((timer.callback.clone(), vec![Value::Number(id)]));
            timer.fired += 1;
            match timer.interval {
                Some(interval) if !interval.is_zero() => timer.due = now + interval,
                _ => remove = true,
            }
        }
        if remove {
            state.m7.timers.remove(&id);
        }
    }
    for channel in state.m7.channels.values_mut() {
        if let Some(rx) = &channel.rx {
            while let Ok(chunk) = rx.try_recv() {
                channel.buffer.extend_from_slice(&chunk);
            }
        }
    }
    state.m7.pumping = false;
    for (callback, args) in callbacks {
        if let Err(e) = crate::script::invoke_callback(ed, env, state, &callback, args) {
            ed.emsg(e.to_string());
        }
    }
    for (buf, lines) in terminal_lines {
        let all = if let Some(terminal) = state.m7.terminals.get_mut(&buf) {
            terminal.lines.extend(lines);
            Some(terminal.lines.clone())
        } else {
            None
        };
        if let Some(lines) = all {
            if ed.current_bufnr() == buf {
                let end = ed.line_count();
                let _ = ed.replace_lines(0, end, lines);
            }
        }
    }
}

fn channel_read(state: &mut ScriptState, id: i64, raw: bool) -> VimResult<Value> {
    let Some(channel) = state.m7.channels.get_mut(&id) else {
        return Ok(Value::string(""));
    };
    if raw {
        let text = String::from_utf8_lossy(&channel.buffer).into_owned();
        channel.buffer.clear();
        return Ok(Value::string(text));
    }
    if let Some(index) = channel.buffer.iter().position(|byte| *byte == b'\n') {
        let mut line: Vec<u8> = channel.buffer.drain(..=index).collect();
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        return Ok(Value::string(String::from_utf8_lossy(&line).into_owned()));
    }
    Ok(Value::string(""))
}

fn channel_write(state: &ScriptState, id: i64, bytes: &[u8]) -> VimResult<()> {
    let Some(channel) = state.m7.channels.get(&id) else {
        return Err(err("E906", "channel is not open"));
    };
    if let Some(job_id) = channel.job {
        let Some(job) = state.m7.jobs.get(&job_id) else {
            return Err(err("E906", "channel is not open"));
        };
        if let Some(stdin) = &job.stdin {
            let mut stdin = stdin
                .lock()
                .map_err(|_| err("E906", "channel is not open"))?;
            stdin
                .write_all(bytes)
                .map_err(|e| err("E906", format!("write failed: {e}")))?;
            let _ = stdin.flush();
            return Ok(());
        }
        return Err(err("E906", "channel has no stdin"));
    }
    if let Some(stream) = &channel.stream {
        let mut stream = stream
            .lock()
            .map_err(|_| err("E906", "channel is not open"))?;
        stream
            .write_all(bytes)
            .map_err(|e| err("E906", format!("write failed: {e}")))?;
        let _ = stream.flush();
        return Ok(());
    }
    Err(err("E906", "channel is not open"))
}

fn ch_open(ed: &mut Editor, state: &mut ScriptState, args: &[Value]) -> VimResult<Value> {
    let address = sarg(args, 0);
    let stream = match TcpStream::connect(&address) {
        Ok(stream) => stream,
        Err(e) => {
            ed.messages
                .info(format!("E475: Can't connect to {address}: {e}"));
            return Ok(Value::Number(0));
        }
    };
    let reader = match stream.try_clone() {
        Ok(reader) => reader,
        Err(e) => {
            ed.messages
                .info(format!("E475: Can't connect to {address}: {e}"));
            return Ok(Value::Number(0));
        }
    };
    let rx = spawn_reader(reader);
    let id = state.m7.alloc_channel();
    state.m7.channels.insert(
        id,
        Channel {
            id,
            job: None,
            stream: Some(Arc::new(Mutex::new(stream))),
            rx: Some(rx),
            buffer: Vec::new(),
            open: true,
            next_msg_id: 0,
            request_id: 0,
        },
    );
    Ok(Value::Number(id))
}

fn timer_start(state: &mut ScriptState, args: &[Value]) -> VimResult<Value> {
    let millis = narg(args, 0).max(0);
    let Some(callback) = args.get(1).cloned() else {
        return Err(err("E119", "Not enough arguments for timer_start"));
    };
    let mut interval = None;
    if let Some(options) = args.get(2).filter(|value| matches!(value, Value::Dict(_))) {
        if let Some(repeat) = dict_get(options, "repeat") {
            let repeat = repeat.to_number().max(0);
            if repeat > 0 {
                interval = Some(Duration::from_millis(repeat as u64));
            }
        }
    }
    let id = state.m7.alloc_timer();
    state.m7.timers.insert(
        id,
        Timer {
            id,
            due: Instant::now() + Duration::from_millis(millis as u64),
            interval,
            callback,
            paused: false,
            fired: 0,
        },
    );
    Ok(Value::Number(id))
}

fn timer_info(state: &mut ScriptState, args: &[Value]) -> Value {
    let filter = args.first().map(Value::to_number);
    let now = Instant::now();
    let mut list = Vec::new();
    for timer in state.m7.timers.values() {
        if let Some(filter) = filter {
            if filter != timer.id {
                continue;
            }
        }
        let mut dict = VimDict::new();
        dict.insert("id", Value::Number(timer.id));
        dict.insert(
            "time",
            Value::Number(timer.due.saturating_duration_since(now).as_millis() as i64),
        );
        dict.insert(
            "repeat",
            Value::Number(timer.interval.map(|d| d.as_millis() as i64).unwrap_or(0)),
        );
        dict.insert("paused", Value::Number(i64::from(timer.paused)));
        list.push(Value::dict(dict));
    }
    Value::list(list)
}

fn terminal_send_keys(state: &mut ScriptState, buf: i32, keys: &str) -> VimResult<()> {
    let Some(terminal) = state.m7.terminals.get(&buf) else {
        return Err(err("E474", "Invalid terminal buffer"));
    };
    let job = terminal.job;
    let Some(job) = state.m7.jobs.get(&job) else {
        return Err(err("E474", "Invalid terminal job"));
    };
    if let Some(stdin) = &job.stdin {
        let mut stdin = stdin
            .lock()
            .map_err(|_| err("E906", "terminal is closed"))?;
        stdin
            .write_all(keys.as_bytes())
            .map_err(|e| err("E906", format!("write failed: {e}")))?;
        let _ = stdin.flush();
    }
    Ok(())
}

/// Dispatch an M7 builtin; None when the name is not handled here.
pub fn call(
    name: &str,
    args: &[Value],
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
) -> Option<VimResult<Value>> {
    let handles = name.starts_with("job_")
        || name.starts_with("ch_")
        || name.starts_with("timer_")
        || name.starts_with("term_")
        || name == "terminalprops";
    if !handles {
        return None;
    }
    Some((|| {
        pump(ed, env, state);
        match name {
            "job_start" => job_start(ed, state, args),
            "job_status" => Ok(Value::string(job_status(state, narg(args, 0)))),
            "job_stop" => {
                let id = narg(args, 0);
                if let Some(job) = state.m7.jobs.get_mut(&id) {
                    if job.status == "run" {
                        if let Ok(mut child) = job.child.lock() {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        job.status = "dead";
                        job.exitval = -1;
                    }
                }
                Ok(Value::Number(0))
            }
            "job_getchannel" => {
                let id = narg(args, 0);
                let channel = state.m7.jobs.get(&id).map(|job| job.channel).unwrap_or(0);
                Ok(Value::Number(channel))
            }
            "job_info" => {
                let filter = args.first().map(Value::to_number);
                let mut list = Vec::new();
                for job in state.m7.jobs.values() {
                    if filter.is_some_and(|id| id != job.id) {
                        continue;
                    }
                    let mut dict = VimDict::new();
                    dict.insert("id", Value::Number(job.id));
                    dict.insert("status", Value::string(job.status));
                    dict.insert("exitval", Value::Number(job.exitval));
                    dict.insert("cmd", Value::string(job.program.clone()));
                    dict.insert(
                        "args",
                        Value::list(
                            job.args
                                .iter()
                                .map(|arg| Value::string(arg.clone()))
                                .collect(),
                        ),
                    );
                    dict.insert("channel", Value::Number(job.channel));
                    list.push(Value::dict(dict));
                }
                if filter.is_some() {
                    Ok(list
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| Value::dict(VimDict::new())))
                } else {
                    Ok(Value::list(list))
                }
            }
            "job_setoptions" => Ok(Value::Number(0)),
            "ch_open" => ch_open(ed, state, args),
            "ch_close" | "ch_close_in" => {
                let id = narg(args, 0);
                if let Some(channel) = state.m7.channels.get_mut(&id) {
                    channel.open = false;
                    channel.stream = None;
                }
                Ok(Value::Number(0))
            }
            "ch_status" => {
                let id = narg(args, 0);
                let status = match state.m7.channels.get(&id) {
                    Some(channel) => {
                        if let Some(job_id) = channel.job {
                            match state.m7.jobs.get(&job_id) {
                                Some(job) if job.status == "run" => "open",
                                Some(_) => "closed",
                                None => "fail",
                            }
                        } else if channel.open {
                            "open"
                        } else {
                            "closed"
                        }
                    }
                    None => "fail",
                };
                Ok(Value::string(status))
            }
            "ch_sendraw" | "ch_sendexpr" => {
                let id = narg(args, 0);
                let payload = if name == "ch_sendexpr" {
                    let value = args.get(1).cloned().unwrap_or(Value::Null);
                    let message_id = state
                        .m7
                        .channels
                        .get_mut(&id)
                        .map(|channel| {
                            channel.next_msg_id += 1;
                            channel.next_msg_id
                        })
                        .unwrap_or(1);
                    format!("[{message_id},{}]\n", crate::script::json_encode(&value)?)
                } else {
                    sarg(args, 1)
                };
                channel_write(state, id, payload.as_bytes())?;
                Ok(Value::Number(0))
            }
            "ch_read" | "ch_readraw" => {
                let id = narg(args, 0);
                let raw = name == "ch_readraw";
                let timeout = args
                    .get(1)
                    .and_then(|options| dict_get(options, "timeout"))
                    .map(|value| value.to_number().max(0) as u64)
                    .unwrap_or(0);
                pump(ed, env, state);
                let mut value = channel_read(state, id, raw)?;
                let deadline = Instant::now() + Duration::from_millis(timeout);
                while value.to_string_value().is_empty() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                    pump(ed, env, state);
                    value = channel_read(state, id, raw)?;
                }
                Ok(value)
            }
            "ch_evalraw" => {
                let id = narg(args, 0);
                let payload = sarg(args, 1);
                channel_write(state, id, payload.as_bytes())?;
                for _ in 0..100 {
                    pump(ed, env, state);
                    let value = channel_read(state, id, true)?;
                    if !value.to_string_value().is_empty() {
                        return Ok(value);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(Value::string(""))
            }
            "ch_evalexpr" => {
                let id = narg(args, 0);
                let value = args.get(1).cloned().unwrap_or(Value::Null);
                let message_id = state
                    .m7
                    .channels
                    .get_mut(&id)
                    .map(|channel| {
                        channel.request_id += 1;
                        channel.request_id
                    })
                    .unwrap_or(1);
                let payload = format!("[{message_id},{}]\n", crate::script::json_encode(&value)?);
                channel_write(state, id, payload.as_bytes())?;
                for _ in 0..100 {
                    pump(ed, env, state);
                    let line = channel_read(state, id, false)?;
                    let text = line.to_string_value();
                    if !text.is_empty() {
                        let decoded = crate::script::json_decode(&text)?;
                        if let Value::List(items) = &decoded {
                            let items = items.borrow();
                            if items.len() >= 2 && items[0].to_number() == message_id {
                                return Ok(items[1].clone());
                            }
                        }
                        return Ok(decoded);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(Value::Null)
            }
            "ch_canread" => {
                let id = narg(args, 0);
                Ok(Value::Number(i64::from(
                    state
                        .m7
                        .channels
                        .get(&id)
                        .is_some_and(|channel| !channel.buffer.is_empty()),
                )))
            }
            "ch_getjob" => {
                let id = narg(args, 0);
                Ok(Value::Number(
                    state.m7.channels.get(&id).and_then(|c| c.job).unwrap_or(0),
                ))
            }
            "ch_getbufnr" => Ok(Value::Number(-1)),
            "ch_info" => {
                let mut dict = VimDict::new();
                if let Some(channel) = state.m7.channels.get(&narg(args, 0)) {
                    dict.insert("id", Value::Number(channel.id));
                    dict.insert(
                        "status",
                        Value::string(if channel.open { "open" } else { "closed" }),
                    );
                }
                Ok(Value::dict(dict))
            }
            "ch_log" | "ch_logfile" | "ch_setoptions" => Ok(Value::Number(0)),
            "timer_start" => timer_start(state, args),
            "timer_stop" => {
                state.m7.timers.remove(&narg(args, 0));
                Ok(Value::Number(0))
            }
            "timer_stopall" => {
                state.m7.timers.clear();
                Ok(Value::Number(0))
            }
            "timer_pause" => {
                let id = narg(args, 0);
                let paused = narg(args, 1) != 0;
                if let Some(timer) = state.m7.timers.get_mut(&id) {
                    timer.paused = paused;
                }
                Ok(Value::Number(0))
            }
            "timer_info" => Ok(timer_info(state, args)),
            "term_start" => term_start(ed, env, state, args),
            "term_wait" => {
                let buf = narg(args, 0) as i32;
                let millis = narg(args, 1).max(0) as u64;
                let deadline = Instant::now() + Duration::from_millis(millis.max(100));
                while Instant::now() < deadline {
                    pump(ed, env, state);
                    if state
                        .m7
                        .terminals
                        .get(&buf)
                        .is_some_and(|terminal| terminal.lines.iter().any(|line| !line.is_empty()))
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(Value::Number(0))
            }
            "term_getline" => {
                let buf = narg(args, 0) as i32;
                let lnum = narg(args, 1).max(1) as usize;
                let line = state
                    .m7
                    .terminals
                    .get(&buf)
                    .and_then(|terminal| terminal.lines.get(lnum - 1))
                    .cloned()
                    .unwrap_or_default();
                Ok(Value::string(line))
            }
            "term_getsize" => {
                let buf = narg(args, 0) as i32;
                let (rows, cols) = state
                    .m7
                    .terminals
                    .get(&buf)
                    .map(|terminal| (terminal.rows, terminal.cols))
                    .unwrap_or((24, 80));
                Ok(Value::list(vec![Value::Number(rows), Value::Number(cols)]))
            }
            "term_getstatus" => {
                let buf = narg(args, 0) as i32;
                let status = state
                    .m7
                    .terminals
                    .get(&buf)
                    .and_then(|terminal| state.m7.jobs.get(&terminal.job))
                    .map(|job| {
                        if job.status == "run" {
                            "running"
                        } else {
                            "finished"
                        }
                    })
                    .unwrap_or("finished");
                Ok(Value::string(status))
            }
            "term_getjob" => {
                let buf = narg(args, 0) as i32;
                Ok(Value::Number(
                    state
                        .m7
                        .terminals
                        .get(&buf)
                        .map(|terminal| terminal.job)
                        .unwrap_or(0),
                ))
            }
            "term_sendkeys" => {
                terminal_send_keys(state, narg(args, 0) as i32, &sarg(args, 1))?;
                Ok(Value::Number(0))
            }
            "term_list" => Ok(Value::list(
                state
                    .m7
                    .terminals
                    .keys()
                    .map(|buf| Value::Number(i64::from(*buf)))
                    .collect(),
            )),
            "term_scrape" => {
                let buf = narg(args, 0) as i32;
                let lnum = narg(args, 1).max(1) as usize;
                let line = state
                    .m7
                    .terminals
                    .get(&buf)
                    .and_then(|terminal| terminal.lines.get(lnum - 1))
                    .cloned()
                    .unwrap_or_default();
                let cells = line
                    .chars()
                    .map(|ch| {
                        let mut dict = VimDict::new();
                        dict.insert("chars", Value::string(ch.to_string()));
                        Value::dict(dict)
                    })
                    .collect();
                Ok(Value::list(cells))
            }
            "terminalprops" => {
                let mut dict = VimDict::new();
                dict.insert("rows", Value::Number(24));
                dict.insert("cols", Value::Number(80));
                dict.insert("term_name", Value::string("xterm"));
                Ok(Value::dict(dict))
            }
            "term_getaltscreen" => Ok(Value::Number(0)),
            "term_getcursor" => Ok(Value::list(vec![Value::Number(1), Value::Number(1)])),
            "term_getscrolled" => Ok(Value::Number(0)),
            "term_gettitle" => Ok(Value::string("")),
            "term_gettty" => Ok(Value::string("")),
            "term_getansicolors" => Ok(Value::list(Vec::new())),
            "term_getattr" => Ok(Value::string("")),
            "term_setansicolors" | "term_setapi" | "term_setkill" | "term_setrestore"
            | "term_setsize" | "term_dumpdiff" | "term_dumpload" | "term_dumpwrite" => {
                Ok(Value::Number(0))
            }
            other => Err(err("E117", format!("Unknown function: {other}"))),
        }
    })())
}

/// `:terminal [cmd]`: open a terminal buffer and switch to it.
pub fn cmd_terminal(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &crate::parser::ParsedEx,
) -> VimResult<crate::exec::ExOutcome> {
    let command = p.args.trim();
    let value = if command.is_empty() {
        Value::Null
    } else {
        Value::string(command.to_string())
    };
    let buf = term_start(ed, env, state, &[value])?.to_number();
    if buf > 0 {
        ed.switch_buffer(buf as i32)?;
    }
    Ok(crate::exec::ExOutcome::Continue)
}

pub fn term_start(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    args: &[Value],
) -> VimResult<Value> {
    let job = job_start(ed, state, args)?.to_number();
    if job == 0 {
        return Ok(Value::Number(0));
    }
    let buf = ed.new_buffer();
    if let Ok(buffer) = ed.buffer_mut(buf) {
        let _ = buffer.set_line_text(0, String::new());
    }
    let mut rows = 24;
    let mut cols = 80;
    if let Some(options) = args.get(1).filter(|value| matches!(value, Value::Dict(_))) {
        if let Some(value) = dict_get(options, "rows") {
            rows = value.to_number().max(1);
        }
        if let Some(value) = dict_get(options, "cols") {
            cols = value.to_number().max(1);
        }
    }
    state.m7.terminals.insert(
        buf,
        Terminal {
            job,
            lines: Vec::new(),
            rows,
            cols,
        },
    );
    if let Some(job) = state.m7.jobs.get_mut(&job) {
        job.terminal = Some(buf);
    }
    let _ = env;
    Ok(Value::Number(i64::from(buf)))
}
