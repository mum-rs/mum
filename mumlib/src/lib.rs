pub mod command;
pub mod error;
pub mod state;

use colored::*;
use log::*;

pub fn setup_logger() {
    fern::Dispatch::new()
        .format(|out, message, record| {
            let message = message.to_string();
            out.finish(format_args!(
                "{} {}:{}{}{}",
                //TODO runtime flag that disables color
                match record.level() {
                    Level::Error => "ERROR".red(),
                    Level::Warn => "WARN ".yellow(),
                    Level::Info => "INFO ".normal(),
                    Level::Debug => "DEBUG".green(),
                    Level::Trace => "TRACE".normal(),
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
        .chain(std::io::stderr())
        .apply()
        .unwrap();
}
