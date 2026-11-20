//! M7: jobs, timers, channels and terminal buffers.

use std::io::{Read, Write};
use std::net::TcpListener;

use rvim_app::Session;

#[test]
fn job_output_callback_and_status() {
    let mut s = Session::new();
    s.ex("let g:lines = []").unwrap();
    let command = if cfg!(windows) {
        "cmd /C echo hello"
    } else {
        "sh -c 'echo hello'"
    };
    s.ex(&format!(
        "let g:job = job_start('{command}', {{'out_cb': {{j, m -> add(g:lines, m)}}}})"
    ))
    .unwrap();
    let id = s.viml.env.get_var("g:job").unwrap().to_number();
    assert!(id > 0, "job id");
    s.ex("sleep 300m").unwrap();
    let lines = s.viml.env.get_var("g:lines").unwrap();
    let rvim_viml::Value::List(list) = lines else {
        panic!("lines is not a list");
    };
    assert!(
        list.borrow()
            .iter()
            .any(|line| line.to_string_value().contains("hello")),
        "output callback fired: {:?}",
        list.borrow()
    );
    s.ex("sleep 200m").unwrap();
    s.ex(&format!("let g:status = job_status({id})")).unwrap();
    let status = s.viml.env.get_var("g:status").unwrap().to_string_value();
    assert!(status == "dead" || status == "run", "status {status}");
}

#[test]
fn job_string_callback_from_a_sourced_function() {
    let dir = std::env::temp_dir().join("rvim_m7_cb");
    let _ = std::fs::create_dir_all(&dir);
    let script = dir.join("cb.vim");
    std::fs::write(
        &script,
        "function! Handler(job, msg)\n  call add(g:lines, a:msg)\nendfunction\n",
    )
    .unwrap();
    let mut s = Session::new();
    s.ex("let g:lines = []").unwrap();
    s.source(&script).unwrap();
    let command = if cfg!(windows) {
        "cmd /C echo sourced"
    } else {
        "sh -c 'echo sourced'"
    };
    s.ex(&format!(
        "let g:job = job_start('{command}', {{'out_cb': 'Handler'}})"
    ))
    .unwrap();
    s.ex("sleep 300m").unwrap();
    let lines = s.viml.env.get_var("g:lines").unwrap();
    let rvim_viml::Value::List(list) = lines else {
        panic!("lines is not a list");
    };
    assert!(
        list.borrow()
            .iter()
            .any(|line| line.to_string_value().contains("sourced")),
        "sourced callback fired: {:?}",
        list.borrow()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn timers_fire_repeatedly_and_stop() {
    let mut s = Session::new();
    s.ex("let g:ticks = []").unwrap();
    s.ex("let g:t = timer_start(10, {id -> add(g:ticks, id)})")
        .unwrap();
    s.ex("sleep 100m").unwrap();
    let ticks = s.viml.env.get_var("g:ticks").unwrap();
    let rvim_viml::Value::List(list) = ticks else {
        panic!("ticks is not a list");
    };
    let fired = list.borrow().len();
    assert!(
        fired >= 1,
        "timer fired {fired} times; messages: {:?}",
        s.editor.messages.history()
    );
    let timer = s.viml.env.get_var("g:t").unwrap().to_number();
    s.ex(&format!("call timer_stop({timer})")).unwrap();
    let after_stop = list.borrow().len();
    s.ex("sleep 80m").unwrap();
    assert_eq!(list.borrow().len(), after_stop);
}

#[test]
fn tcp_channel_sends_and_receives() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 64];
        let n = stream.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..n]).to_string();
        stream.write_all(b"reply\n").unwrap();
        request
    });
    let mut s = Session::new();
    s.ex(&format!("let g:ch = ch_open('127.0.0.1:{port}')"))
        .unwrap();
    let channel = s.viml.env.get_var("g:ch").unwrap().to_number();
    assert!(channel > 0, "channel id");
    s.ex(&format!("call ch_sendraw({channel}, 'ping')"))
        .unwrap();
    let request = server.join().unwrap();
    assert!(request.contains("ping"), "server saw {request:?}");
    s.ex("sleep 100m").unwrap();
    s.ex(&format!("let g:got = ch_read({channel})")).unwrap();
    assert_eq!(
        s.viml.env.get_var("g:got").unwrap().to_string_value(),
        "reply"
    );
}

#[test]
fn channel_evalexpr_round_trips_json() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 128];
        let n = stream.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..n]).to_string();
        stream.write_all(b"[1,\"pong\"]\n").unwrap();
        request
    });
    let mut s = Session::new();
    s.ex(&format!("let g:ch = ch_open('127.0.0.1:{port}')"))
        .unwrap();
    let channel = s.viml.env.get_var("g:ch").unwrap().to_number();
    s.ex(&format!("let g:reply = ch_evalexpr({channel}, 'ping')"))
        .unwrap();
    let request = server.join().unwrap();
    assert!(request.contains("[1,\"ping\"]"), "request {request:?}");
    assert_eq!(
        s.viml.env.get_var("g:reply").unwrap().to_string_value(),
        "pong"
    );
}

#[test]
fn terminal_buffer_captures_output() {
    let command = if cfg!(windows) {
        "cmd /C echo terminal"
    } else {
        "sh -c 'echo terminal'"
    };
    let mut s = Session::new();
    s.ex(&format!("let g:buf = term_start('{command}')"))
        .unwrap();
    let buf = s.viml.env.get_var("g:buf").unwrap().to_number();
    assert!(buf > 0, "terminal buffer");
    s.ex(&format!("call term_wait({buf}, 500)")).unwrap();
    s.ex(&format!("let g:line = term_getline({buf}, 1)"))
        .unwrap();
    assert!(
        s.viml
            .env
            .get_var("g:line")
            .unwrap()
            .to_string_value()
            .contains("terminal"),
        "terminal output"
    );
}

#[test]
fn has_reports_job_channel_and_terminal() {
    let mut s = Session::new();
    for feature in ["job", "channel", "terminal", "timers"] {
        s.ex(&format!("let g:h = has('{feature}')")).unwrap();
        assert_eq!(
            s.viml.env.get_var("g:h").unwrap().to_number(),
            1,
            "{feature}"
        );
    }
}
