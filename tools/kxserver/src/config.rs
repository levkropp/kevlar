// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Command-line parsing.
//
// No clap dependency — the CLI surface is tiny and hand-rolled parsing keeps
// the binary small and the error messages tailored to our diagnostic use.
//
// Recognized invocations:
//
//     kxserver :N [options]
//         N is the display number.  Default is :1.
//
//     kxserver --help
//     kxserver -h
//
// Options:
//     --log=SPEC           Log filter.  SPEC is a comma-separated list of:
//                              trace | req | rep | evt | err | warn  (min severity)
//                              op=NN[,NN...]                         (restrict to opcodes)
//                              client=N[,N...]                       (restrict to clients)
//                          Default: req.
//     --dump-to=PATH       Append raw byte stream of all requests/replies to PATH.
//     --no-listen-tcp      Refuse TCP connections (we never listen on TCP anyway,
//                          this flag is accepted for xinit compatibility).
//     --nocursor           Do not draw a software cursor (accepted for compat).

use crate::log::{Filter, OpSet, Sev};

#[derive(Debug, Clone)]
pub struct Config {
    pub display: u16,
    pub log_filter: Filter,
    pub dump_to: Option<String>,
    pub ppm_on_exit: Option<String>,
    /// Test-only synthetic input injection.  Each entry is a raw
    /// `InputEvent` to enqueue after every accepted connection
    /// (resolved by the server every poll iteration).  Syntax:
    ///   motion:dx:dy
    ///   button:N:down    (or :up)
    ///   key:KEYCODE:down (or :up)
    /// Multiple --inject flags may be given.
    pub inject: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            display: 1,
            log_filter: Filter {
                min_sev: Sev::Req,
                opcodes: OpSet::all(),
                clients: None,
            },
            dump_to: None,
            ppm_on_exit: None,
            inject: Vec::new(),
        }
    }
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut cfg = Config::default();
    for arg in args {
        if arg == "-h" || arg == "--help" {
            print_help();
            std::process::exit(0);
        } else if let Some(rest) = arg.strip_prefix(':') {
            cfg.display = rest.parse::<u16>()
                .map_err(|_| format!("invalid display number: {arg}"))?;
        } else if let Some(spec) = arg.strip_prefix("--log=") {
            cfg.log_filter = parse_log_spec(spec)?;
        } else if let Some(path) = arg.strip_prefix("--dump-to=") {
            cfg.dump_to = Some(path.to_string());
        } else if let Some(path) = arg.strip_prefix("--ppm-on-exit=") {
            cfg.ppm_on_exit = Some(path.to_string());
        } else if let Some(spec) = arg.strip_prefix("--inject=") {
            cfg.inject.push(spec.to_string());
        } else if arg == "--no-listen-tcp" || arg == "-nolisten" {
            // accepted for compat, no effect
        } else if arg == "--nocursor" {
            // accepted for compat, no effect
        } else if arg.starts_with("vt") || arg == "-keeptty" || arg == "-novtswitch" {
            // xinit passes "vt1" etc.  Ignore.
        } else {
            return Err(format!("unrecognized argument: {arg}"));
        }
    }
    Ok(cfg)
}

fn parse_log_spec(spec: &str) -> Result<Filter, String> {
    let mut filter = Filter::default_trace();
    // Default to Req severity unless the spec specifies otherwise.
    filter.min_sev = Sev::Req;
    let mut opcodes = OpSet::none();
    let mut any_opcode = false;
    let mut clients: Vec<u32> = Vec::new();
    let mut any_client = false;
    for token in spec.split(',') {
        let token = token.trim();
        if token.is_empty() { continue; }
        match token {
            "trace" => filter.min_sev = Sev::Trace,
            "req"   => filter.min_sev = Sev::Req,
            "rep"   => filter.min_sev = Sev::Rep,
            "evt"   => filter.min_sev = Sev::Evt,
            "err"   => filter.min_sev = Sev::Err,
            "warn"  => filter.min_sev = Sev::Warn,
            _ => {
                if let Some(op) = token.strip_prefix("op=") {
                    any_opcode = true;
                    let n: u8 = op.parse()
                        .map_err(|_| format!("invalid opcode in --log: {op}"))?;
                    opcodes.insert(n);
                } else if let Some(c) = token.strip_prefix("client=") {
                    any_client = true;
                    let n: u32 = c.parse()
                        .map_err(|_| format!("invalid client id in --log: {c}"))?;
                    clients.push(n);
                } else {
                    return Err(format!("unknown log-spec token: {token}"));
                }
            }
        }
    }
    filter.opcodes = if any_opcode { opcodes } else { OpSet::all() };
    filter.clients = if any_client { Some(clients) } else { None };
    Ok(filter)
}

fn print_help() {
    let help = "\
kxserver — minimal diagnostic X11 display server for Kevlar.

Usage: kxserver [:N] [options]

Arguments:
  :N                 Display number (default :1).

Options:
  --log=SPEC         Log filter. SPEC is a comma-separated list of:
                       trace|req|rep|evt|err|warn  — minimum severity
                       op=NN[,NN...]               — restrict to opcodes
                       client=N[,N...]             — restrict to clients
                     Default: req.
  --dump-to=PATH     Append raw wire bytes to PATH (post-mortem diff vs xtrace).
  --no-listen-tcp    Accepted for compatibility; we never listen on TCP.
  --nocursor         Accepted for compatibility.
  -h, --help         Show this help and exit.
";
    eprint!("{help}");
}
