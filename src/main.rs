use crate::channel_list::ChannelList;
use crate::enums::*;
use crate::server::{Server, SERVER_ADDR};
use crate::shared_stdout::SharedStdout;
use clap::{Arg, ArgAction, Command};
use std::fs;
use std::process;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio::time::{sleep, Duration};

pub mod channel_list;
pub mod enums;
pub mod server;
pub mod shared_stdout;

const TWITCH_IRC_ADDR: &str = "irc.chat.twitch.tv:6667";

#[tokio::main]
async fn main() -> Result<(), IrcError> {
    let args = cmdline();

    if args.is_server {
        if let Err(e) = main_loop(&args).await {
            eprintln!("Error: {}", e);
        }
    } else {
        if let Err(e) = do_client_commands(&args).await {
            eprintln!("Error: {}", e);
        }
    }

    Ok(())
}

async fn main_loop(args: &MainArgs) -> Result<(), IrcError> {
    let (irc_tx, mut irc_rx) = broadcast::channel::<Vec<u8>>(32);
    let (timed_tx, timed_rx) = mpsc::channel::<Vec<u8>>(1024);
    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(32);
    let sstdout = SharedStdout::new();

    // consume initial broadcast recv handle
    tokio::spawn(async move { while let Ok(_) = irc_rx.recv().await {} });

    start_timed_irc(timed_rx, irc_tx.clone()).await;

    let join_tx = ChannelList::start(timed_tx.clone(), sstdout.clone()).await?;
    Server::start(output_rx, timed_tx.clone(), join_tx.clone()).await?;

    // the actual main loop: connect to twitch IRC and process I/O, reconnect on error
    let mut first_loop = true;
    loop {
        let socket = match connect(args).await {
            Ok(sock) => sock,
            Err(_) => {
                sstdout
                    .write_str("failed to connect, attempting again in 5 seconds...\n")
                    .await;
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        sstdout.write_str("connected to twitch\n").await;

        let (mut reader, mut writer) = socket.into_split();
        let mut irc_rx = irc_tx.subscribe();
        let mut buf = [0u8; 4096];

        tokio::spawn(async move {
            loop {
                let buf = match irc_rx.recv().await {
                    Ok(b) => b,
                    Err(_) => break,
                };
                if let Err(_) = writer.write_all(&buf).await {
                    break;
                }
            }
            writer.forget();
        });

        // join (or rejoin) channels
        if first_loop {
            first_loop = false;
            for ch in &args.join_channels {
                join_tx.send(JoinCmd::Join(ch.clone())).await?;
            }
        } else {
            join_tx.send(JoinCmd::JoinAll).await?;
        }

        loop {
            let n = match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };

            let c = buf[0];
            match c {
                // PING
                b'P' => {
                    if let Err(_) = irc_tx.send(b"PONG :tmi.twitch.tv\r\n".to_vec()) {
                        break;
                    }
                }
                // DISCONNECT
                b'D' => break,
                // app-level messages
                _ => {
                    if let Err(_) = output_tx.send((&buf[0..n]).to_vec()).await {
                        break;
                    }
                }
            }
        }
        drop(reader);

        sstdout
            .write_str("disconnected, attempting to reconnect in 5 seconds...\n")
            .await;
        sleep(Duration::from_secs(5)).await;
    }
}

async fn connect(args: &MainArgs) -> Result<TcpStream, IrcError> {
    let mut addr_iter = lookup_host(TWITCH_IRC_ADDR).await?;
    let addr = addr_iter.next().ok_or(IrcError::MustReconnect)?;
    let mut sock = TcpStream::connect(addr).await?;

    let oauth = fs::read_to_string(&args.oauth_path).expect("Could not find oauth file");

    let ident = format!(
        "PASS {}\r\nNICK {}\r\nCAP REQ :twitch.tv/tags\r\nCAP REQ :twitch.tv/commands\r\n",
        oauth,
        args.name.to_lowercase()
    );
    match sock.write_all(ident.as_bytes()).await {
        Ok(_) => Ok(sock),
        Err(e) => {
            _ = sock.shutdown().await;
            Err(IrcError::IoError(e))
        }
    }
}

async fn start_timed_irc(
    mut timed_rx: mpsc::Receiver<Vec<u8>>,
    irc_tx: broadcast::Sender<Vec<u8>>,
) {
    tokio::spawn(async move {
        while let Some(msg) = timed_rx.recv().await {
            irc_tx.send(msg)?;
            sleep(Duration::from_millis(1500)).await;
        }
        Ok::<(), IrcError>(())
    });
}

async fn do_client_commands(args: &MainArgs) -> Result<(), IrcError> {
    let mut addr_iter = lookup_host(SERVER_ADDR).await?;
    let addr = addr_iter.next().ok_or(IrcError::MustReconnect)?;
    let mut sock = TcpStream::connect(addr).await?;

    for ch in &args.join_channels {
        let j = format!(
            "{}\"json_meta_type\":\"join\",\"channel_name\":\"{}\"{}\n",
            '{', ch, '}'
        );
        sock.write_all(j.as_bytes()).await?;
    }

    sleep(Duration::from_secs(1)).await;
    Ok(())
}

fn cmdline() -> MainArgs {
    let mut cmd = Command::new("rust-twitch_chatbot_server")
        .arg(
            Arg::new("name")
                .long("name")
                .short('n')
                .help("login name to use for connecting to Twitch"),
        )
        .arg(
            Arg::new("oauth")
                .long("oauth")
                .short('o')
                .help("path to file containing oauth token to use for login"),
        )
        .arg(
            Arg::new("join")
                .long("join")
                .short('j')
                .action(ArgAction::Append)
                .help("channel to join, may be specified multiple times"),
        );

    let mut ret = MainArgs {
        name: "".to_string(),
        oauth_path: "".to_string(),
        join_channels: Vec::new(),
        is_server: false,
    };

    let usage = cmd.render_help();
    let m = cmd.get_matches();

    if let Some(name) = m.get_one::<String>("name") {
        ret.name = name.clone();
    }

    if let Some(oauth) = m.get_one::<String>("oauth") {
        ret.oauth_path = oauth.clone();
    }

    ret.join_channels = m
        .get_many::<String>("join")
        .unwrap_or_default()
        .map(|s| s.to_string())
        .collect::<Vec<String>>();

    if ret.name.is_empty() && ret.oauth_path.is_empty() && ret.join_channels.is_empty() {
        eprintln!("{}", usage);
        process::exit(1);
    }

    if !ret.name.is_empty() && !ret.oauth_path.is_empty() {
        ret.is_server = true;
    } else if ret.join_channels.is_empty() {
        eprintln!(
            "Error: must specify --name and --oauth to run in server mode,\n       \
                   OR --join to command a running server to join channels\n"
        );
        eprintln!("{}", usage);
        process::exit(1);
    }

    ret
}

struct MainArgs {
    name: String,
    oauth_path: String,
    join_channels: Vec<String>,
    is_server: bool,
}
