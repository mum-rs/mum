#![warn(elided_lifetimes_in_paths)]
#![warn(meta_variable_misuse)]
#![warn(missing_debug_implementations)]
#![warn(single_use_lifetimes)]
#![warn(unreachable_pub)]
#![warn(unused_crate_dependencies)]
#![warn(unused_import_braces)]
#![warn(unused_lifetimes)]
#![warn(unused_qualifications)]
// #![warn(missing_docs)] may be enabled later when more is documented
#![deny(macro_use_extern_crate)]
#![deny(missing_abi)]
#![deny(future_incompatible)]
#![forbid(unsafe_code)]
#![forbid(non_ascii_idents)]
//! Shared items for crates that want to communicate with mumd and/or mumctl.


pub mod command;
pub mod config;
pub mod error;
pub mod state;

pub use error::Error;

use colored::*;
use log::*;

/// The default file path to use for the socket.
pub const SOCKET_PATH: &str = "/tmp/mumd";

/// The default mumble port.
pub const DEFAULT_PORT: u16 = 64738;

/// Setup a minimal fern logger.
///
/// Format: `LEVEL [yyyy-mm-dd][HH:MM:SS] FILE:LINE MESSAGE`
pub fn setup_logger<T: Into<fern::Output>>(target: T, color: bool) {
    fern::Dispatch::new()
        .format(move |out, message, record| {
            let message = message.to_string();
            out.finish(format_args!(
                "{} {} {}:{}{}{}",
                //TODO runtime flag that disables color
                if color {
                    match record.level() {
                        Level::Error => "ERROR".red(),
                        Level::Warn => "WARN ".yellow(),
                        Level::Info => "INFO ".normal(),
                        Level::Debug => "DEBUG".green(),
                        Level::Trace => "TRACE".normal(),
                    }
                } else {
                    match record.level() {
                        Level::Error => "ERROR",
                        Level::Warn => "WARN ",
                        Level::Info => "INFO ",
                        Level::Debug => "DEBUG",
                        Level::Trace => "TRACE",
                    }
                    .normal()
                },
                chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S%.6f]"),
                record.file().unwrap(),
                record.line().unwrap(),
                if message.chars().any(|e| e == '\n') {
                    "\n"
                } else {
                    " "
                },
                message
            ))
        })
        .level(log::LevelFilter::Debug)
        .chain(target.into())
        .apply()
        .unwrap();
}
