use futures_util::FutureExt;
use log::error;
use mum::state::State;
use mumlib::command::{Command, CommandResponse};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use tokio::select;
use tokio::sync::mpsc;

const INDENTATION: &str = "  ";

#[tokio::main]
async fn main() {
    let state = match State::new() {
        Ok(s) => s,
        Err(e) => {
            error!("Error instantiating mumd: {}", e);
            return;
        }
    };

    let (command_sender, command_receiver) = mpsc::unbounded_channel();

    let run = select! {
        r = mum::client::handle(state, command_receiver).fuse() => r,
        _ = handle_repl(command_sender).fuse() => Ok(()),
    };

    if let Err(e) = run {
        error!("{}", e);
    }
}

async fn handle_repl(
    command_sender: mpsc::UnboundedSender<(
        Command,
        mpsc::UnboundedSender<mumlib::error::Result<Option<CommandResponse>>>,
    )>,
) {
    let mut rl = Editor::<()>::new();
    loop {
        let readline;
        (rl, readline) = tokio::task::spawn_blocking(move || {
            let readline = rl.readline(">> ");
            (rl, readline)
        })
        .await
        .unwrap();

        match readline {
            Ok(line) => {
                rl.add_history_entry(line.as_str());
                let sender = command_sender.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                    let command = Command::Status;
                    let (tx, mut rx) = mpsc::unbounded_channel();
                    sender.send((command, tx)).unwrap();
                    println!("{:?}", rx.recv().await);
                });
            }
            Err(ReadlineError::Interrupted) => {
                println!("CTRL-C");
                break;
            }
            Err(ReadlineError::Eof) => {
                println!("CTRL-D");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }
    }
}
