use mum::cli::{CliError, Mum, match_args};
use mumlib::command::{Command as MumCommand, CommandResponse};
use serde::de::DeserializeOwned;
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::os::unix::net::UnixStream;
use structopt::StructOpt;
use tokio::select;
use tokio::sync::mpsc;

#[allow(unused_imports)]
use log::{debug, error, info, warn};


type CommandSender = mpsc::UnboundedSender<(
    MumCommand,
    mpsc::UnboundedSender<mumlib::error::Result<Option<CommandResponse>>>,
)>;

#[tokio::main]
async fn main() {
    mumlib::setup_logger(std::io::stderr(), true);

    if std::env::args().any(|s| s.as_str() == "--version" || s.as_str() == "-V") {
        println!("mumctl {}", env!("VERSION"));
        return;
    }
    color_eyre::install().unwrap();

    let args = Mum::from_args();

    // create communication channels:

    // match_args -> output (terminal)
    let (lines_tx, mut lines_rx) = mpsc::unbounded_channel();
    // match_args -> mumd (final destination mum)
    let (command_tx, mut command_rx): (CommandSender, _) = mpsc::unbounded_channel();

    // spawn the glue that
    // - receives commands from match_args and sends them to mumd
    // - receives responses from mumd and sends them to match_args
    // - receives lines from match_args and prints them
    // TODO: match_args needs a better name. it is too central. cli::handle?
    tokio::spawn(async move {
        loop {
            select! {
                Some(line) = lines_rx.recv() => {
                    println!("{line}");
                }
                Some((command, response_tx)) = command_rx.recv() => {
                    // send command to mumd and receive some responses
                    // it is perfectly fine to block here
                    let mut responses = send_command(command).unwrap();
                    while let Some(response) = responses.next() {
                        debug!("got response {:?}", response);
                        response_tx.send(response).unwrap();
                    }
                }
                else => {
                    // if we don't have any more lines to print *and* no more commands to send, call
                    // it a day.
                    break;
                }
            }
        }
    });

    // parse args
    if let Err(e) = match_args(args, command_tx, lines_tx.clone()).await {
        error!("{}", e);
    }
}

/// Tries to find a running mumd instance and send a single command to it. Returns an iterator which
/// yields all responses that mumd sends for that particular command.
fn send_command(
    command: MumCommand,
) -> Result<impl Iterator<Item = mumlib::error::Result<Option<CommandResponse>>>, CliError> {
    let mut connection =
        UnixStream::connect(mumlib::SOCKET_PATH).map_err(|_| CliError::ConnectionError)?;

    let serialized = bincode::serialize(&command).unwrap();

    connection
        .write(&(serialized.len() as u32).to_be_bytes())
        .map_err(|_| CliError::ConnectionError)?;
    connection
        .write(&serialized)
        .map_err(|_| CliError::ConnectionError)?;

    connection
        .shutdown(std::net::Shutdown::Write)
        .map_err(|_| CliError::ConnectionError)?;

    Ok(BincodeIter::new(connection))
}

/// A struct to represent an iterator that deserializes bincode-encoded data from a `Reader`.
struct BincodeIter<R, I> {
    reader: R,
    phantom: PhantomData<*const I>,
}

impl<R, I> BincodeIter<R, I> {
    /// Creates a new `BincodeIter` from a reader.
    fn new(reader: R) -> Self {
        Self {
            reader,
            phantom: PhantomData,
        }
    }
}

impl<R, I> Iterator for BincodeIter<R, I>
where
    R: Read,
    I: DeserializeOwned,
{
    type Item = I;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.reader.read_exact(&mut [0; 4]).ok()?;
        bincode::deserialize_from(&mut self.reader).ok()
    }
}

