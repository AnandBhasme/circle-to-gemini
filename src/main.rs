// Circle to Gemini — Main entry point
// A lightweight Rust desktop tool for KDE Plasma 6 / Wayland
// that lets you select any area on screen and query Gemini AI about it.

#![allow(dead_code)]

#[macro_use]
mod logger;
mod app;
mod capture;
mod config;
mod gemini;
mod lens;
mod overlay;
mod tray;

use std::env;
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

const SOCKET_PATH: &str = "/tmp/circle-to-gemini.sock";

fn main() -> iced::Result {
    let cfg = config::Config::load();
    logger::init(cfg.debug_logging);
    log_info!("main", "Circle to Gemini starting up, debug_logging={}", cfg.debug_logging);

    let args: Vec<String> = env::args().collect();
    let is_trigger = args.iter().any(|arg| arg == "--trigger" || arg == "-t" || arg == "--capture");

    // If an instance is already running, send capture trigger and exit
    if try_send_trigger() {
        println!("Sent capture trigger to running Circle to Gemini instance.");
        log_info!("main", "Sent capture trigger to running daemon instance and exiting.");
        return Ok(());
    }

    if is_trigger {
        println!("No running instance found. Starting daemon and triggering capture...");
    }

    // Clean up stale socket if present
    if Path::new(SOCKET_PATH).exists() {
        let _ = std::fs::remove_file(SOCKET_PATH);
    }

    // Channel for triggering capture from tray or Unix socket
    let (trigger_tx, trigger_rx) = mpsc::unbounded_channel();

    // Spawn IPC socket listener in background
    let socket_tx = trigger_tx.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            listen_ipc(socket_tx).await;
        });
    });

    // Spawn tray icon in background
    let tray_tx = trigger_tx.clone();
    std::thread::spawn(move || {
        tray::run_tray_with_sender(tray_tx);
    });

    // If started with --trigger and no prior instance was running, trigger capture immediately
    if is_trigger {
        let _ = trigger_tx.send(tray::TrayCommand::Activate);
    }

    // Run the iced daemon (no window on startup — tray/hotkey opens overlay)
    iced::daemon("Circle to Gemini", app::App::update, app::App::view)
        .subscription(app::App::subscription)
        .theme(app::App::theme)
        .run_with(move || app::App::new(trigger_rx))
}

/// Send a trigger command to an already running instance over Unix socket
fn try_send_trigger() -> bool {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(_) => return false,
    };

    rt.block_on(async {
        if let Ok(mut stream) = UnixStream::connect(SOCKET_PATH).await {
            let _ = stream.write_all(b"capture\n").await;
            true
        } else {
            false
        }
    })
}

/// Background IPC socket listener for incoming hotkey triggers
async fn listen_ipc(tx: mpsc::UnboundedSender<tray::TrayCommand>) {
    let listener = match UnixListener::bind(SOCKET_PATH) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind IPC socket {}: {}", SOCKET_PATH, e);
            return;
        }
    };

    loop {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 64];
            if let Ok(n) = stream.read(&mut buf).await {
                let cmd = String::from_utf8_lossy(&buf[..n]);
                log_debug!("ipc", "Received IPC socket message: {}", cmd.trim());
                if cmd.contains("capture") {
                    let res = tx.send(tray::TrayCommand::Activate);
                    log_info!("ipc", "Triggered screen capture via IPC, success: {}", res.is_ok());
                }
            }
        }
    }
}
