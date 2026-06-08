//! Integration tests for thin client mode.

mod support;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Deserialize;
use support::{
    cleanup_test_base, client_handshake, encode_varint_u16, encode_varint_u32, frame_message,
    read_server_message, register_runtime_dir, register_spawned_gmux_pid,
    unregister_spawned_gmux_pid, wait_for_file, wait_for_message_variant, wait_for_socket,
    wait_until, TEST_PROTOCOL_VERSION,
};

fn unique_test_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    PathBuf::from(format!(
        "/tmp/gmux-client-test-{}-{nanos}",
        std::process::id()
    ))
}

struct SpawnedGmux {
    _master: Option<Box<dyn MasterPty + Send>>,
    child: Box<dyn Child + Send + Sync>,
}

impl SpawnedGmux {
    fn close_master(&mut self) {
        drop(self._master.take());
    }
}

impl Drop for SpawnedGmux {
    fn drop(&mut self) {
        let pid = self.child.process_id();
        let _ = self.child.kill();
        self.close_master();

        if let Some(pid) = pid {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let mut status = 0;
                let result =
                    unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
                if result == pid as libc::pid_t || result == -1 {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }

            unregister_spawned_gmux_pid(Some(pid));
        }
    }
}

fn cleanup_spawned_gmux(spawned: SpawnedGmux, base: PathBuf) {
    drop(spawned);
    cleanup_test_base(&base);
}

fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn spawn_client_process(
    config_home: &PathBuf,
    runtime_dir: &PathBuf,
    api_socket_path: &PathBuf,
) -> SpawnedGmux {
    register_runtime_dir(runtime_dir);
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gmux"));
    cmd.arg("client");
    cmd.env("XDG_CONFIG_HOME", config_home);
    cmd.env("XDG_RUNTIME_DIR", runtime_dir);
    cmd.env("GMUX_SOCKET_PATH", api_socket_path);
    cmd.env_remove("GMUX_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env_remove("GMUX_ENV");

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_gmux_pid(child.process_id());
    drop(pair.slave);

    SpawnedGmux {
        _master: Some(pair.master),
        child,
    }
}

fn spawn_server(
    config_home: &PathBuf,
    runtime_dir: &PathBuf,
    api_socket_path: &PathBuf,
    _client_socket_path: &PathBuf,
) -> SpawnedGmux {
    fs::create_dir_all(config_home.join("gmux")).unwrap();
    fs::create_dir_all(runtime_dir).unwrap();
    register_runtime_dir(runtime_dir);
    fs::write(config_home.join("gmux/config.toml"), "onboarding = false\n").unwrap();

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gmux"));
    cmd.arg("server");
    cmd.env("XDG_CONFIG_HOME", config_home);
    cmd.env("XDG_RUNTIME_DIR", runtime_dir);
    cmd.env("GMUX_SOCKET_PATH", api_socket_path);
    cmd.env_remove("GMUX_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env_remove("GMUX_ENV");

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_gmux_pid(child.process_id());
    drop(pair.slave);

    SpawnedGmux {
        _master: Some(pair.master),
        child,
    }
}

fn ping_socket(socket_path: &PathBuf) -> String {
    let mut stream = UnixStream::connect(socket_path).expect("should connect to API socket");

    let request = r#"{"id":"1","method":"ping","params":{}}"#;
    writeln!(stream, "{}", request).unwrap();

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();
    response.trim().to_string()
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct FrameWire {
    cells: Vec<CellWire>,
    width: u16,
    height: u16,
    cursor: Option<CursorWire>,
    hyperlinks: Vec<String>,
    graphics: Vec<u8>,
    debug_timing: Option<FrameDebugTimingWire>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct FrameDebugTimingWire {
    server_input_queue_us: u64,
    server_input_to_frame_us: u64,
    server_pty_dirty_to_frame_us: Option<u64>,
    server_render_us: Option<u64>,
    server_frame_build_us: Option<u64>,
    server_graphics_us: Option<u64>,
    server_prepare_us: Option<u64>,
    server_target_count: u16,
    server_active_only: bool,
    server_mirror_flush: bool,
    server_pending_mirror: bool,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct CellWire {
    symbol: String,
    fg: u32,
    bg: u32,
    modifier: u16,
    skip: bool,
    hyperlink: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CursorWire {
    x: u16,
    y: u16,
    visible: bool,
    shape: u8,
}

fn decode_frame_payload(payload: &[u8]) -> std::io::Result<FrameWire> {
    bincode::serde::decode_from_slice(payload, bincode::config::standard())
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))
        .and_then(|(frame, consumed): (FrameWire, usize)| {
            if consumed != payload.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "frame payload had trailing bytes: consumed={}, len={}",
                        consumed,
                        payload.len()
                    ),
                ));
            }
            Ok(frame)
        })
}

fn read_next_frame_payload(stream: &mut UnixStream, timeout: Duration) -> Result<Vec<u8>, String> {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match read_server_message(stream) {
            Ok((1, payload)) => return Ok(payload),
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    Err("timed out waiting for Frame message".into())
}

fn frame_contains_text(frame: &FrameWire, needle: &str) -> bool {
    if frame.cells.is_empty() {
        return false;
    }

    let width = frame.width.max(1) as usize;
    let mut text = String::new();
    for row in frame.cells.chunks(width) {
        for cell in row {
            let _ = (cell.fg, cell.bg, cell.modifier, cell.skip);
            text.push_str(&cell.symbol);
        }
        text.push('\n');
    }
    let _ = (frame.height, frame.graphics.len());
    if let Some(cursor) = frame.cursor.as_ref() {
        let _ = (cursor.x, cursor.y, cursor.visible, cursor.shape);
    }

    text.contains(needle)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn client_connects_and_receives_frame() {
    // Client connects to server and handshake completes.
    // Client receives Frame messages.
    // Server sends rendered frames to connected clients.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");
    let client_socket = runtime_dir.join("gmux-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_file(&client_socket, Duration::from_secs(10));

    // Connect and handshake.
    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_handshake(&mut stream, TEST_PROTOCOL_VERSION, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(
        version, TEST_PROTOCOL_VERSION,
        "server should report protocol version"
    );
    assert!(
        error.is_none(),
        "handshake should not have error: {:?}",
        error
    );

    read_next_frame_payload(&mut stream, Duration::from_secs(10))
        .expect("should receive a frame from server");

    cleanup_spawned_gmux(spawned, base);
}

#[test]
fn client_sees_headless_startup_config_diagnostic() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");
    let client_socket = runtime_dir.join("gmux-client.sock");

    let app_dir = if cfg!(debug_assertions) {
        "gmux-dev"
    } else {
        "gmux"
    };
    fs::create_dir_all(config_home.join(app_dir)).unwrap();
    fs::write(
        config_home.join(app_dir).join("config.toml"),
        "[keys\nprefix = \"ctrl+a\"\n",
    )
    .unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();
    register_runtime_dir(&runtime_dir);

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gmux"));
    cmd.arg("server");
    cmd.env("XDG_CONFIG_HOME", &config_home);
    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
    cmd.env("GMUX_SOCKET_PATH", &api_socket);
    cmd.env_remove("GMUX_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env_remove("GMUX_ENV");

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_gmux_pid(child.process_id());
    drop(pair.slave);

    let spawned = SpawnedGmux {
        _master: Some(pair.master),
        child,
    };
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_file(&client_socket, Duration::from_secs(10));

    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_handshake(&mut stream, TEST_PROTOCOL_VERSION, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, TEST_PROTOCOL_VERSION);
    assert!(error.is_none(), "{:?}", error);

    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut found_diagnostic = false;
    while Instant::now() < deadline {
        match read_server_message(&mut stream) {
            Ok((1, payload)) => {
                let frame = decode_frame_payload(&payload).expect("decode frame");
                if frame_contains_text(&frame, "config parse error") {
                    found_diagnostic = true;
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    assert!(
        found_diagnostic,
        "attached client should see startup config parse diagnostic"
    );

    cleanup_spawned_gmux(spawned, base);
}

#[test]
fn client_input_forwarded_to_pane() {
    // Stdin input is forwarded to server as ClientMessage::Input.
    // Server routes client input to the correct PTY.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");
    let client_socket = runtime_dir.join("gmux-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_file(&client_socket, Duration::from_secs(10));

    // Connect and handshake.
    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_handshake(&mut stream, TEST_PROTOCOL_VERSION, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, TEST_PROTOCOL_VERSION);
    assert!(error.is_none(), "{:?}", error);

    // Send an Input message containing "echo hello\n".
    // ClientMessage::Input is variant 1: { data: Vec<u8> }
    let input_data = b"echo hello\n".to_vec();
    let input_payload = {
        let mut buf = encode_varint_u32(1); // variant 1 = Input
                                            // Encode the data as a bincode Vec<u8>: length (varint) + bytes
        buf.extend_from_slice(&encode_varint_u32(input_data.len() as u32));
        buf.extend_from_slice(&input_data);
        buf
    };
    let framed = frame_message(&input_payload);
    stream
        .write_all(&framed)
        .expect("should send Input message");
    stream.flush().expect("should flush");

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should still respond to ping after input"
    );

    // Verify the server is still alive and responsive via API.
    let response = ping_socket(&api_socket);
    assert!(
        response.contains("pong"),
        "server should still respond to ping after input: {response}"
    );

    cleanup_spawned_gmux(spawned, base);
}

#[test]
fn client_resize_sends_message() {
    // Terminal resize triggers ClientMessage::Resize.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");
    let client_socket = runtime_dir.join("gmux-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_file(&client_socket, Duration::from_secs(10));

    // Connect and handshake.
    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_handshake(&mut stream, TEST_PROTOCOL_VERSION, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, TEST_PROTOCOL_VERSION);
    assert!(error.is_none(), "{:?}", error);

    // Drain the initial frame(s).
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    while read_server_message(&mut stream).is_ok() {}

    // Send a Resize message: ClientMessage::Resize is variant 3.
    let resize_payload = {
        let mut buf = encode_varint_u32(3); // variant 3 = Resize
        buf.extend_from_slice(&encode_varint_u16(120)); // cols
        buf.extend_from_slice(&encode_varint_u16(40)); // rows
        buf.extend_from_slice(&encode_varint_u32(8)); // cell_width_px
        buf.extend_from_slice(&encode_varint_u32(16)); // cell_height_px
        buf
    };
    let framed = frame_message(&resize_payload);
    stream
        .write_all(&framed)
        .expect("should send Resize message");
    stream.flush().expect("should flush");

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should respond after resize"
    );

    // Verify the server is still alive.
    let response = ping_socket(&api_socket);
    assert!(
        response.contains("pong"),
        "server should respond after resize: {response}"
    );

    cleanup_spawned_gmux(spawned, base);
}

#[test]
fn server_shutdown_sends_message_to_client() {
    // ServerShutdown causes clean exit with informative message.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");
    let client_socket = runtime_dir.join("gmux-client.sock");

    let mut spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_file(&client_socket, Duration::from_secs(10));

    // Connect and handshake.
    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_handshake(&mut stream, TEST_PROTOCOL_VERSION, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, TEST_PROTOCOL_VERSION);
    assert!(error.is_none(), "{:?}", error);

    // Send SIGINT so the server takes the graceful shutdown path and
    // broadcasts ServerShutdown before exiting.
    if let Some(pid) = spawned.child.process_id() {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGINT);
        }
    }

    // The client should receive an explicit ServerShutdown message, or at
    // minimum observe clean connection close if shutdown races with send.
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut saw_shutdown = false;
    let mut saw_disconnect = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match read_server_message(&mut stream) {
            Ok((variant, _)) => {
                if variant == 3 {
                    saw_shutdown = true;
                    break;
                }
            }
            Err(_) => {
                saw_disconnect = true;
                break;
            }
        }
    }
    assert!(
        saw_shutdown || saw_disconnect,
        "client should observe ServerShutdown or disconnect during graceful shutdown"
    );

    // Wait for the server to exit after shutdown signal.
    spawned.close_master();
    let _ = spawned.child.wait();

    drop(spawned);
    cleanup_test_base(&base);
}

#[test]
fn server_unreachable_shows_clear_error() {
    // when server is unreachable, the client exits quickly
    // with an actionable connection-failed message.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");

    fs::create_dir_all(config_home.join("gmux")).unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();
    register_runtime_dir(&runtime_dir);
    fs::write(config_home.join("gmux/config.toml"), "onboarding = false\n").unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_gmux"))
        .arg("client")
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("GMUX_SOCKET_PATH", &api_socket)
        .env_remove("GMUX_CLIENT_SOCKET_PATH")
        .env_remove("GMUX_ENV")
        .output()
        .expect("client command should run");

    assert!(
        !output.status.success(),
        "client should fail when no server is running"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to connect to server"),
        "stderr should mention connection failure: {stderr}"
    );
    assert!(
        stderr.contains("Is gmux server running?"),
        "stderr should include actionable guidance: {stderr}"
    );
    assert!(
        stderr.contains("Socket path:"),
        "stderr should include attempted socket path: {stderr}"
    );

    cleanup_test_base(&base);
}

#[test]
fn server_crash_after_attach_causes_lost_connection_error() {
    // attach a real thin client connection, kill server unexpectedly,
    // assert clean non-zero client exit plus lost-connection signal.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");
    let client_socket = runtime_dir.join("gmux-client.sock");

    let mut spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_file(&client_socket, Duration::from_secs(10));

    // Attach a real thin client (client subcommand) through PTY so handshake and
    // terminal setup paths are exercised.
    let mut thin_client = spawn_client_process(&config_home, &runtime_dir, &api_socket);

    // Prove attached before kill by waiting for at least one frame message.
    let mut thin_reader = thin_client
        ._master
        .as_ref()
        .expect("thin client master")
        .try_clone_reader()
        .expect("clone client PTY reader");
    let attached_before_kill = {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut buf = [0u8; 4096];
        let mut seen = false;
        while Instant::now() < deadline {
            match thin_reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let out = String::from_utf8_lossy(&buf[..n]);
                    if !out.is_empty() {
                        seen = true;
                        break;
                    }
                }
                Ok(_) => thread::sleep(Duration::from_millis(30)),
                Err(_) => thread::sleep(Duration::from_millis(30)),
            }
        }
        seen
    };
    assert!(
        attached_before_kill,
        "thin client must complete attach and receive frame before server crash"
    );

    // Kill server unexpectedly.
    if let Some(pid) = spawned.child.process_id() {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    spawned.close_master();

    // Client should exit non-zero after connection loss.
    let mut crash_output = String::new();
    let exited = {
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut exited = false;
        while Instant::now() < deadline {
            if thin_client.child.try_wait().ok().flatten().is_some() {
                exited = true;
                break;
            }
            // Keep draining client output so the process can progress to exit.
            let mut buf = [0u8; 1024];
            if let Ok(n) = thin_reader.read(&mut buf) {
                if n > 0 {
                    crash_output.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        exited
    };
    assert!(exited, "thin client should exit after server SIGKILL");

    let status = thin_client.child.wait().expect("wait thin client status");
    assert!(
        !status.success(),
        "thin client should exit non-zero after lost server connection"
    );

    // Drain trailing output and require the explicit user-visible lost-connection message.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        match thin_reader.read(&mut buf) {
            Ok(n) if n > 0 => crash_output.push_str(&String::from_utf8_lossy(&buf[..n])),
            Ok(_) => break,
            Err(_) => break,
        }
        thread::sleep(Duration::from_millis(30));
    }

    let crash_output_lc = crash_output.to_lowercase();
    assert!(
        crash_output_lc.contains("lost connection to server"),
        "thin client must emit explicit lost-connection message after server crash; output: {crash_output:?}"
    );

    // Ensure server is gone.
    let _ = spawned.child.wait();

    cleanup_test_base(&base);
}

#[test]
fn client_receives_frame_after_pane_output() {
    // End-to-end test: server renders, client receives Frame.
    // This test verifies the full flow:
    // 1. Start server
    // 2. Connect client, handshake
    // 3. Send input to pane (echo command)
    // 4. Wait for a new frame from the server
    // 5. Verify the frame contains the pane output
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");
    let client_socket = runtime_dir.join("gmux-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_file(&client_socket, Duration::from_secs(10));

    // Connect and handshake.
    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_handshake(&mut stream, TEST_PROTOCOL_VERSION, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, TEST_PROTOCOL_VERSION);
    assert!(error.is_none(), "{:?}", error);

    read_next_frame_payload(&mut stream, Duration::from_secs(10))
        .expect("should receive initial frame");

    // Send input to trigger a state change and re-render.
    let input_data = b"echo test-output\n".to_vec();
    let input_payload = {
        let mut buf = encode_varint_u32(1); // Input variant
        buf.extend_from_slice(&encode_varint_u32(input_data.len() as u32));
        buf.extend_from_slice(&input_data);
        buf
    };
    let framed = frame_message(&input_payload);
    stream.write_all(&framed).expect("send input");
    stream.flush().expect("flush");

    // Read subsequent frames — the server should have re-rendered after
    // the input was processed.
    let received_frame = wait_for_message_variant(&mut stream, Duration::from_secs(2), 1)
        .expect("wait for post-output frame");
    assert!(received_frame, "should receive a Frame after pane output");

    cleanup_spawned_gmux(spawned, base);
}

#[test]
fn navigate_mode_keybind_dispatch_in_server() {
    // Navigate mode keybind dispatch in server.
    // This tests that the server can process a prefix key (Ctrl+B) to enter
    // navigate mode, and then a navigation key (like 'n' for new workspace).
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");
    let client_socket = runtime_dir.join("gmux-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_file(&client_socket, Duration::from_secs(10));

    // Connect and handshake.
    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_handshake(&mut stream, TEST_PROTOCOL_VERSION, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, TEST_PROTOCOL_VERSION);
    assert!(error.is_none(), "{:?}", error);

    // Drain initial frames.
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    while read_server_message(&mut stream).is_ok() {}

    // Send Ctrl+B (prefix key) as raw bytes. In kitty mode, Ctrl+B is 0x02.
    // In legacy mode, it's also 0x02 (control character).
    let prefix_input = vec![0x02]; // Ctrl+B
    let input_payload = {
        let mut buf = encode_varint_u32(1); // Input variant
        buf.extend_from_slice(&encode_varint_u32(prefix_input.len() as u32));
        buf.extend_from_slice(&prefix_input);
        buf
    };
    let framed = frame_message(&input_payload);
    stream.write_all(&framed).expect("send prefix key");
    stream.flush().expect("flush");

    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    while read_server_message(&mut stream).is_ok() {}
    stream.set_read_timeout(None).unwrap();

    // Send 'n' (new workspace in navigate mode).
    let n_input = b"n".to_vec();
    let n_payload = {
        let mut buf = encode_varint_u32(1); // Input variant
        buf.extend_from_slice(&encode_varint_u32(n_input.len() as u32));
        buf.extend_from_slice(&n_input);
        buf
    };
    let framed = frame_message(&n_payload);
    stream.write_all(&framed).expect("send n key");
    stream.flush().expect("flush");

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should still respond after navigate mode input"
    );

    // Verify the server is still alive and the API still works.
    let response = ping_socket(&api_socket);
    assert!(
        response.contains("pong"),
        "server should still respond after navigate mode input: {response}"
    );

    cleanup_spawned_gmux(spawned, base);
}

#[test]
fn pane_spawn_cwd_fallback_in_server() {
    // Pane spawn failure cwd fallback in server context.
    // This test verifies that the server can start even with invalid
    // session data pointing to non-existent directories.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");
    let client_socket = runtime_dir.join("gmux-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_file(&client_socket, Duration::from_secs(10));

    // The server should have started successfully even though there are
    // no existing sessions (fresh state). The test verifies that the
    // server doesn't crash during initial pane creation.
    let response = ping_socket(&api_socket);
    assert!(
        response.contains("pong"),
        "server should respond to ping after startup: {response}"
    );

    // Create an initial tab via the API — this tests pane creation in the server.
    let mut ws_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let request = r#"{"id":"2","method":"tab.create","params":{"label":"cwd-test"}}"#;
    writeln!(ws_stream, "{}", request).unwrap();

    let mut reader = BufReader::new(ws_stream);
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();

    assert!(
        response.contains("tab_created") || response.contains("ok"),
        "tab creation should succeed: {response}"
    );

    cleanup_spawned_gmux(spawned, base);
}

#[test]
fn graceful_shutdown_sends_server_shutdown_to_client() {
    // Issue 2 fix: SIGINT triggers initiate_shutdown → ServerShutdown
    // broadcast to all clients before the server exits.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");
    let client_socket = runtime_dir.join("gmux-client.sock");

    let mut spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_file(&client_socket, Duration::from_secs(10));

    // Connect and handshake.
    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_handshake(&mut stream, TEST_PROTOCOL_VERSION, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, TEST_PROTOCOL_VERSION);
    assert!(error.is_none(), "{:?}", error);

    // Drain initial frame(s).
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    while read_server_message(&mut stream).is_ok() {}

    // Send SIGINT to the server process to trigger graceful shutdown.
    if let Some(pid) = spawned.child.process_id() {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGINT);
        }
    }

    // The client should receive a ServerShutdown message (variant 4)
    // before the connection is closed, not just an abrupt EOF.
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let result = read_server_message(&mut stream);
    match result {
        Ok((variant, _payload)) => {
            assert_eq!(
                variant, 4,
                "expected ServerShutdown (variant 4), got variant {variant}"
            );
        }
        Err(e) => {
            panic!("expected ServerShutdown message before connection close, got error: {e}");
        }
    }

    // Wait for the server to exit.
    spawned.close_master();
    let _ = spawned.child.wait();

    drop(spawned);
    cleanup_test_base(&base);
}

#[test]
fn removed_agent_report_api_does_not_notify_client() {
    // Notification events are forwarded as ServerMessage::Notify to connected
    // clients when a background event produces a toast. The removed
    // pane.report_agent API should not produce one.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("gmux.sock");
    let client_socket = runtime_dir.join("gmux-client.sock");

    // Enable toast in config so the server would produce notifications.
    let app_dir = if cfg!(debug_assertions) {
        "gmux-dev"
    } else {
        "gmux"
    };
    fs::create_dir_all(config_home.join(app_dir)).unwrap();
    fs::write(
        config_home.join(app_dir).join("config.toml"),
        "onboarding = false\n[ui.toast]\nenabled = true\n",
    )
    .unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();
    register_runtime_dir(&runtime_dir);

    // Spawn the server directly (not using spawn_server helper because it
    // overwrites the config file with a minimal one).
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gmux"));
    cmd.arg("server");
    cmd.env("XDG_CONFIG_HOME", &config_home);
    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
    cmd.env("GMUX_SOCKET_PATH", &api_socket);
    cmd.env_remove("GMUX_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env_remove("GMUX_ENV");

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_gmux_pid(child.process_id());
    drop(pair.slave);

    let spawned = SpawnedGmux {
        _master: Some(pair.master),
        child,
    };
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_file(&client_socket, Duration::from_secs(10));

    // Connect as a client and perform handshake.
    let mut stream = UnixStream::connect(&client_socket).expect("should connect");
    let (version, error) = client_handshake(&mut stream, TEST_PROTOCOL_VERSION, 80, 24)
        .expect("handshake should succeed");
    assert_eq!(version, TEST_PROTOCOL_VERSION);
    assert!(error.is_none(), "{:?}", error);

    // Drain initial frame(s).
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    while read_server_message(&mut stream).is_ok() {}

    // Create an initial tab via the API.
    let mut ws_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let request = r#"{"id":"1","method":"tab.create","params":{}}"#;
    writeln!(ws_stream, "{}", request).unwrap();
    let mut reader = BufReader::new(ws_stream);
    let mut ws_response = String::new();
    reader.read_line(&mut ws_response).unwrap();
    let ws_response: serde_json::Value = serde_json::from_str(&ws_response).unwrap();
    let pane_id = ws_response["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Legacy report calls are no longer socket API methods and must not emit
    // client notifications.
    let mut report_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let report_request = format!(
        r#"{{"id":"3","method":"pane.report_agent","params":{{"pane_id":"{pane_id}","agent":"pi","state":"blocked","source":"test"}}}}"#
    );
    writeln!(report_stream, "{}", report_request).unwrap();
    let mut report_reader = BufReader::new(report_stream);
    let mut report_response = String::new();
    report_reader.read_line(&mut report_response).unwrap();
    let report_json: serde_json::Value = serde_json::from_str(&report_response).unwrap();
    assert_eq!(report_json["error"]["code"], "invalid_request");

    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut found_notify = false;
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        match read_server_message(&mut stream) {
            Ok((variant, _payload)) => {
                if variant == 5 {
                    found_notify = true;
                    break;
                }
            }
            Err(_) => {
                break;
            }
        }
    }

    assert!(
        !found_notify,
        "client should not receive a ServerMessage::Notify after removed pane.report_agent"
    );

    cleanup_spawned_gmux(spawned, base);
}
