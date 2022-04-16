use futures_util::FutureExt;
use mum::cli::{Mum, match_args};
use mum::state::State;
use mumlib::command::{Command as MumCommand, CommandResponse};
use readline_async::Editor;
use structopt::StructOpt;
use tokio::select;
use tokio::sync::mpsc;

#[allow(unused_imports)]
use log::{debug, error, info, warn};

#[tokio::main]
async fn main() {
    mum::notifications::init();
    color_eyre::install().unwrap();

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

type CommandSender = mpsc::UnboundedSender<(
    MumCommand,
    mpsc::UnboundedSender<mumlib::error::Result<Option<CommandResponse>>>,
)>;

async fn handle_repl(command_sender: CommandSender) {
    let (mut editor, lines) = Editor::new();
    readline_async::enable_raw_mode().unwrap();
    loop {
        let (line, err);
        (editor, (line, err)) = tokio::task::spawn(async move {
            let readline = editor.readline().await;
            (editor, readline)
        })
        .await
        .unwrap();

        if let Err(e) = err {
            readline_async::disable_raw_mode().unwrap();
            println!("\n{e:?}");
            break;
        }
        let sender = command_sender.clone();
        let (output_tx, mut output_rx) = mpsc::unbounded_channel();

        lines.unbounded_send(format!(">> {}", line)).unwrap(); // TODO
        let mut args = shell_words::split(&line).unwrap();
        args.insert(0, String::from("mumrepl"));
        let args = match Mum::from_iter_safe(args) {
            Ok(args) => args,
            Err(e) => {
                lines.unbounded_send(format!("command error: {}", e)).unwrap();
                continue;
            }
        };

        if let Err(e) = match_args(args, sender, output_tx).await {
            lines.unbounded_send(format!("mum error: {}", e)).unwrap();
        }

        let lines = lines.clone();
        tokio::spawn(async move {
            while let Some(line) = output_rx.recv().await {
                lines.unbounded_send(line).unwrap(); // TODO
            }
        });
    }
}
