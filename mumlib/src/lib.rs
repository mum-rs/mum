pub mod command;
pub mod config;
pub mod error;
pub mod state;

use colored::*;
use log::*;

pub const SOCKET_PATH: &str = "/var/tmp/mumd";
pub const DEFAULT_PORT: u16 = 64738;

pub fn setup_logger<T: Into<fern::Output>>(target: T, color: bool) {
    fern::Dispatch::new()
        .format(move |out, message, record| {
            let message = message.to_string();
            out.finish(format_args!(
                "{} {}:{}{}{}",
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
