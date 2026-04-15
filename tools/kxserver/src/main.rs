// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// kxserver — minimal diagnostic X11 display server for the Kevlar kernel.
//
// Phase 1: wire protocol + connection setup.  The server binds a Unix socket
// at /tmp/.X11-unix/Xn, accepts clients, parses the handshake, and replies
// with a fixed ConnectionSetup success.  Every opcode that expects a reply
// returns BadImplementation for now.  This is enough for `xlsclients` or
// `xdpyinfo` to complete the handshake and print the server info.

mod atom;
mod client;
mod colormap;
mod config;
mod dispatch;
mod device;
mod event;
mod fb;
mod font;
mod font_data;
mod gc;
mod input;
mod keymap;
mod log;
mod pixmap;
mod property;
mod region;
mod render;
mod render_ext;
mod resources;
mod server;
mod setup;
mod state;
mod window;
mod wire;

use server::{RunError, Server};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let cfg = match config::parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[kxserver] config error: {e}");
            std::process::exit(2);
        }
    };

    log::init(cfg.log_filter.clone(), cfg.dump_to.as_deref());

    log::info(format_args!(
        "phase=9 display=:{} log=configured dump={} ppm_on_exit={} inject={}",
        cfg.display,
        cfg.dump_to.as_deref().unwrap_or("<none>"),
        cfg.ppm_on_exit.as_deref().unwrap_or("<none>"),
        cfg.inject.len(),
    ));

    server::install_shutdown_handlers();

    let mut server = match Server::bind(cfg.display) {
        Ok(s) => s,
        Err(RunError::BindFailed(msg)) => {
            log::fatal(format_args!("bind failed: {msg}"));
            std::process::exit(1);
        }
        Err(_) => {
            log::fatal(format_args!("bind failed (unknown)"));
            std::process::exit(1);
        }
    };
    if !cfg.inject.is_empty() {
        server.inject_events(&cfg.inject);
    }

    let run_result = server.run();

    if let Some(path) = cfg.ppm_on_exit.as_deref() {
        match server.framebuffer_mut().dump_ppm(path) {
            Ok(()) => log::info(format_args!("wrote final framebuffer to {path}")),
            Err(e) => log::warn(format_args!("ppm dump failed: {e}")),
        }
    }

    match run_result {
        Ok(()) => {}
        Err(RunError::Interrupted) => {
            log::info(format_args!("interrupted; shutting down"));
        }
        Err(RunError::PollFailed(msg)) => {
            log::fatal(format_args!("poll failed: {msg}"));
            std::process::exit(1);
        }
        Err(_) => {
            log::fatal(format_args!("run error"));
            std::process::exit(1);
        }
    }
}
