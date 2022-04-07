use futures_util::FutureExt;
use mum::cli::Error;
use mum::cli::Mum;
use mum::state::State;
use mumlib::command::CommandResponse;
use mumlib::config;
use rustyline::error::ReadlineError;
use rustyline::Editor;
use tokio::select;
use tokio::sync::mpsc;

use colored::Colorize;
use log::{error, warn, Level, LevelFilter, Metadata, Record};
use mum::cli::{Channel, CliError, Command, Completions, Server, Target};
use mumlib::command::{ChannelTarget, Command as MumCommand, MessageTarget};
use mumlib::config::{Config, ServerConfig};
use mumlib::state::Channel as MumChannel;
use serde::de::DeserializeOwned;
use std::fmt;
use std::io::{self, BufRead, Read, Write};
use std::iter;
use std::marker::PhantomData;
use std::os::unix::net::UnixStream;
use std::thread;
use structopt::{clap::Shell, StructOpt};

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
        MumCommand,
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
                let mut args = shell_words::split(&line).unwrap();
                args.insert(0, String::from("mumrepl"));
                let opt = Mum::from_iter_safe(args);
                println!("{:?}", opt);
                // tokio::spawn(async move {
                //     tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                //     let command = MumCommand::Status;
                //     let (tx, mut rx) = mpsc::unbounded_channel();
                //     sender.send((command, tx)).unwrap();
                //     println!("{:?}", rx.recv().await);
                // });
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

//**************************************************
// BAD IDEA
// LET'S NOT MERGE THIS
//**************************************************

// fn match_opt(opt: Mum) -> Result<(), Error> {
//     let mut config = config::read_cfg(&config::default_cfg_path())?;
//
//     match opt.command {
//         Command::Connect {
//             host,
//             username,
//             password,
//             port,
//             accept_invalid_cert: cli_accept_invalid_cert,
//         } => {
//             let port = port.unwrap_or(mumlib::DEFAULT_PORT);
//
//             let (host, username, password, port, server_accept_invalid_cert) =
//                 match config.servers.iter().find(|e| e.name == host) {
//                     Some(server) => (
//                         &server.host,
//                         server
//                             .username
//                             .as_ref()
//                             .or_else(|| username.as_ref())
//                             .ok_or(CliError::NoUsername)?,
//                         server.password.as_ref().or_else(|| password.as_ref()),
//                         server.port.unwrap_or(port),
//                         server.accept_invalid_cert,
//                     ),
//                     None => (
//                         &host,
//                         username.as_ref().ok_or(CliError::NoUsername)?,
//                         password.as_ref(),
//                         port,
//                         None,
//                     ),
//                 };
//
//             let config_accept_invalid_cert =
//                 server_accept_invalid_cert.or(config.allow_invalid_server_cert);
//             let specified_accept_invalid_cert =
//                 cli_accept_invalid_cert || config_accept_invalid_cert.is_some();
//
//             let response = send_command(MumCommand::ServerConnect {
//                 host: host.to_string(),
//                 port,
//                 username: username.to_string(),
//                 password: password.map(|x| x.to_string()),
//                 accept_invalid_cert: cli_accept_invalid_cert
//                     || config_accept_invalid_cert.unwrap_or(false),
//             })?;
//             match response {
//                 Ok(Some(CommandResponse::ServerConnect {
//                     welcome_message,
//                     server_state,
//                 })) => {
//                     parse_state(&server_state);
//                     if let Some(message) = welcome_message {
//                         println!("\nWelcome: {}", message);
//                     }
//                 }
//                 Err(mumlib::error::Error::ServerCertReject) => {
//                     error!("Connection rejected since the server supplied an invalid certificate.");
//                     if !specified_accept_invalid_cert {
//                         eprintln!("help: If you trust this server anyway, you can do any of the following to connect:");
//                         eprintln!("  1. Temporarily trust this server by passing --accept-invalid-cert when connecting.");
//                         eprintln!("  2. Permanently trust this server by setting accept_invalid_cert=true in the server's config.");
//                         eprintln!("  3. Permantently trust all invalid certificates by setting accept_all_invalid_certs=true globally");
//                     }
//                 }
//                 Ok(other) => unreachable!(
//                     "Response should only be a ServerConnect or ServerCertReject. Got {:?}",
//                     other
//                 ),
//                 Err(e) => return Err(e.into()),
//             }
//         }
//         Command::Disconnect => {
//             send_command(MumCommand::ServerDisconnect)??;
//         }
//         Command::Server(server_command) => {
//             match_server_command(server_command, &mut config)?;
//         }
//         Command::Channel(channel_command) => {
//             match channel_command {
//                 Channel::List { short: _short } => {
//                     //TODO actually use this
//                     match send_command(MumCommand::ChannelList)?? {
//                         Some(CommandResponse::ChannelList { channels }) => {
//                             print_channel(&channels, 0);
//                         }
//                         _ => unreachable!("Response should only be a ChannelList"),
//                     }
//                 }
//                 Channel::Connect { name } => {
//                     send_command(MumCommand::ChannelJoin {
//                         channel_identifier: name,
//                     })??;
//                 }
//             }
//         }
//         Command::Status => match send_command(MumCommand::Status)?? {
//             Some(CommandResponse::Status { server_state }) => {
//                 parse_state(&server_state);
//             }
//             _ => unreachable!("Response should only be a Status"),
//         },
//         Command::Config { key, value } => match key.as_str() {
//             "audio.input_volume" => {
//                 if let Ok(volume) = value.parse() {
//                     send_command(MumCommand::InputVolumeSet(volume))??;
//                     config.audio.input_volume = Some(volume);
//                 }
//             }
//             "audio.output_volume" => {
//                 if let Ok(volume) = value.parse() {
//                     send_command(MumCommand::OutputVolumeSet(volume))??;
//                     config.audio.output_volume = Some(volume);
//                 }
//             }
//             "accept_all_invalid_certs" => {
//                 if let Ok(b) = value.parse() {
//                     config.allow_invalid_server_cert = Some(b);
//                 }
//             }
//             _ => {
//                 return Err(CliError::ConfigKeyNotFound(key).into());
//             }
//         },
//         Command::ConfigReload => {
//             send_command(MumCommand::ConfigReload)??;
//         }
//         Command::Completions(completions) => {
//             Mum::clap().gen_completions_to(
//                 "mumctl",
//                 match completions {
//                     Completions::Bash => Shell::Bash,
//                     Completions::Fish => Shell::Fish,
//                     _ => Shell::Zsh,
//                 },
//                 &mut io::stdout(),
//             );
//         }
//         Command::Volume { user, volume } => {
//             if let Some(volume) = volume {
//                 send_command(MumCommand::UserVolumeSet(user, volume))??;
//             } else {
//                 //TODO report volume of user
//                 //     needs work on mumd
//                 warn!("Currently unimplemented");
//             }
//         }
//         Command::Mute { user } => match user {
//             Some(user) => {
//                 send_command(MumCommand::MuteOther(user, Some(true)))??;
//             }
//             None => {
//                 send_command(MumCommand::MuteSelf(Some(true)))??;
//             }
//         },
//         Command::Unmute { user } => match user {
//             Some(user) => {
//                 send_command(MumCommand::MuteOther(user, Some(false)))??;
//             }
//             None => {
//                 send_command(MumCommand::MuteSelf(Some(false)))??;
//             }
//         },
//         Command::Deafen => {
//             send_command(MumCommand::DeafenSelf(Some(true)))??;
//         }
//         Command::Undeafen => {
//             send_command(MumCommand::DeafenSelf(Some(false)))??;
//         }
//         Command::Messages { follow } => {
//             for response in send_command_multi(MumCommand::PastMessages { block: follow })? {
//                 match response {
//                     Ok(Some(CommandResponse::PastMessage { message })) => {
//                         println!(
//                             "[{}] {}: {}",
//                             message.0.format("%d %b %H:%M"),
//                             message.2,
//                             message.1
//                         )
//                     }
//                     Ok(_) => unreachable!("Response should only be a Some(PastMessages)"),
//                     Err(e) => error!("{}", e),
//                 }
//             }
//         }
//         Command::Message(target) => match target {
//             Target::Channel {
//                 message,
//                 recursive,
//                 names,
//             } => {
//                 let msg = MumCommand::SendMessage {
//                     message,
//                     targets: if names.is_empty() {
//                         MessageTarget::Channel(vec![(ChannelTarget::Current, recursive)])
//                     } else {
//                         MessageTarget::Channel(
//                             names
//                                 .into_iter()
//                                 .map(|name| (ChannelTarget::Named(name), recursive))
//                                 .collect(),
//                         )
//                     },
//                 };
//                 send_command(msg)??;
//             }
//             Target::User { message, names } => {
//                 let msg = MumCommand::SendMessage {
//                     message,
//                     targets: MessageTarget::User(names),
//                 };
//                 send_command(msg)??;
//             }
//         },
//         Command::Events { follow } => {
//             for response in send_command_multi(MumCommand::Events { block: follow })? {
//                 match response {
//                     Ok(Some(CommandResponse::Event { event })) => {
//                         println!("{}", event)
//                     }
//                     Ok(_) => unreachable!("Response should only be a Some(Event)"),
//                     Err(e) => error!("{}", e),
//                 }
//             }
//         }
//     }
//
//     let config_path = config::default_cfg_path();
//     if !config_path.exists() {
//         println!(
//             "Config file not found. Create one in {}? [Y/n]",
//             config_path.display(),
//         );
//         let stdin = std::io::stdin();
//         let response = stdin.lock().lines().next();
//         if let Some(Ok(true)) = response.map(|e| e.map(|e| &e == "Y")) {
//             config.write(&config_path, true)?;
//         }
//     } else {
//         config.write(&config_path, false)?;
//     }
//     Ok(())
// }
//
// fn match_server_command(server_command: Server, config: &mut Config) -> Result<(), Error> {
//     match server_command {
//         Server::Config {
//             server_name,
//             key,
//             value,
//         } => {
//             let server_name = match server_name {
//                 Some(server_name) => server_name,
//                 None => {
//                     for server in config.servers.iter() {
//                         println!("{}", server.name);
//                     }
//                     return Ok(());
//                 }
//             };
//             let server = config
//                 .servers
//                 .iter_mut()
//                 .find(|s| s.name == server_name)
//                 .ok_or(CliError::NoServerFound(server_name))?;
//
//             match (key.as_deref(), value) {
//                 (None, _) => {
//                     print!(
//                         "{}{}{}{}{}",
//                         format!("host: {}\n", server.host.to_string()),
//                         server
//                             .port
//                             .map(|s| format!("port: {}\n", s))
//                             .unwrap_or_else(|| "".to_string()),
//                         server
//                             .username
//                             .as_ref()
//                             .map(|s| format!("username: {}\n", s))
//                             .unwrap_or_else(|| "".to_string()),
//                         server
//                             .password
//                             .as_ref()
//                             .map(|s| format!("password: {}\n", s))
//                             .unwrap_or_else(|| "".to_string()),
//                         server
//                             .accept_invalid_cert
//                             .map(|b| format!(
//                                 "accept_invalid_cert: {}\n",
//                                 if b { "true" } else { "false" }
//                             ))
//                             .unwrap_or_else(|| "".to_string()),
//                     );
//                 }
//                 (Some("name"), None) => {
//                     println!("{}", server.name);
//                 }
//                 (Some("host"), None) => {
//                     println!("{}", server.host);
//                 }
//                 (Some("port"), None) => {
//                     println!(
//                         "{}",
//                         server
//                             .port
//                             .ok_or_else(|| CliError::NotSet("port".to_string()))?
//                     );
//                 }
//                 (Some("username"), None) => {
//                     println!(
//                         "{}",
//                         server
//                             .username
//                             .as_ref()
//                             .ok_or_else(|| CliError::NotSet("username".to_string()))?
//                     );
//                 }
//                 (Some("password"), None) => {
//                     println!(
//                         "{}",
//                         server
//                             .password
//                             .as_ref()
//                             .ok_or_else(|| CliError::NotSet("password".to_string()))?
//                     );
//                 }
//                 (Some("accept_invalid_cert"), None) => {
//                     println!(
//                         "{}",
//                         server
//                             .accept_invalid_cert
//                             .map(|b| b.to_string())
//                             .ok_or_else(|| CliError::NotSet("accept_invalid_cert".to_string()))?
//                     );
//                 }
//                 (Some("name"), Some(_)) => {
//                     return Err(CliError::UseServerRename.into());
//                 }
//                 (Some("host"), Some(value)) => {
//                     server.host = value;
//                 }
//                 (Some("port"), Some(value)) => {
//                     server.port = Some(value.parse().unwrap());
//                 }
//                 (Some("username"), Some(value)) => {
//                     server.username = Some(value);
//                 }
//                 (Some("password"), Some(value)) => {
//                     server.password = Some(value);
//                     //TODO ask stdin if empty
//                 }
//                 (Some("accept_invalid_cert"), Some(value)) => match value.parse() {
//                     Ok(b) => server.accept_invalid_cert = Some(b),
//                     Err(e) => warn!("{}", e),
//                 },
//                 (Some(_), _) => {
//                     return Err(CliError::ConfigKeyNotFound(key.unwrap()).into());
//                 }
//             }
//         }
//         Server::Rename { old_name, new_name } => {
//             config
//                 .servers
//                 .iter_mut()
//                 .find(|s| s.name == old_name)
//                 .ok_or(CliError::NoServerFound(old_name))?
//                 .name = new_name;
//         }
//         Server::Add {
//             name,
//             host,
//             port,
//             username,
//             password,
//         } => {
//             if config.servers.iter().any(|s| s.name == name) {
//                 return Err(CliError::ServerAlreadyExists(name).into());
//             } else {
//                 config.servers.push(ServerConfig {
//                     name,
//                     host,
//                     port,
//                     username,
//                     password,
//                     accept_invalid_cert: None,
//                 });
//             }
//         }
//         Server::Remove { name } => {
//             let idx = config
//                 .servers
//                 .iter()
//                 .position(|s| s.name == name)
//                 .ok_or(CliError::NoServerFound(name))?;
//             config.servers.remove(idx);
//         }
//         Server::List => {
//             if config.servers.is_empty() {
//                 return Err(CliError::NoServers.into());
//             }
//
//             let longest = config
//                 .servers
//                 .iter()
//                 .map(|s| s.name.len())
//                 .max()
//                 .unwrap()  // ok since !config.servers.is_empty() above
//                 + 1;
//
//             let queries: Vec<_> = config
//                 .servers
//                 .iter()
//                 .map(|s| {
//                     let query = MumCommand::ServerStatus {
//                         host: s.host.clone(),
//                         port: s.port.unwrap_or(mumlib::DEFAULT_PORT),
//                     };
//                     thread::spawn(move || send_command(query))
//                 })
//                 .collect();
//
//             for (server, response) in config.servers.iter().zip(queries) {
//                 match response.join().unwrap() {
//                     Ok(Ok(Some(response))) => {
//                         if let CommandResponse::ServerStatus {
//                             users, max_users, ..
//                         } = response
//                         {
//                             println!(
//                                 "{0:<1$} [{2:}/{3:}]",
//                                 server.name, longest, users, max_users
//                             );
//                         } else {
//                             unreachable!();
//                         }
//                     }
//                     Ok(Ok(None)) => {
//                         println!("{0:<1$} offline", server.name, longest);
//                     }
//                     Ok(Err(e)) => {
//                         error!("{}", e);
//                         return Err(e.into());
//                     }
//                     Err(e) => {
//                         error!("{}", e);
//                         return Err(e.into());
//                     }
//                 }
//             }
//         }
//     }
//     Ok(())
// }
//
// fn parse_state(server_state: &mumlib::state::Server) {
//     println!(
//         "Connected to {} as {}",
//         server_state.host, server_state.username
//     );
//     let own_channel = server_state
//         .channels
//         .iter()
//         .find(|e| e.users.iter().any(|e| e.name == server_state.username))
//         .unwrap();
//     println!(
//         "Currently in {} with {} other client{}:",
//         own_channel.name,
//         own_channel.users.len() - 1,
//         if own_channel.users.len() == 2 {
//             ""
//         } else {
//             "s"
//         }
//     );
//     println!("{}{}", INDENTATION, own_channel.name);
//     for user in &own_channel.users {
//         println!("{}{}{}", INDENTATION, INDENTATION, user);
//     }
// }
