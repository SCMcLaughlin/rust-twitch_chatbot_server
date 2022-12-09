use crate::enums::*;
use json::JsonValue;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

pub const SERVER_ADDR: &str = "127.0.0.1:44632";

type OwnedReadHalf = tokio::net::tcp::OwnedReadHalf;
type JoinSender = mpsc::Sender<JoinCmd>;
type VecSender = mpsc::Sender<Vec<u8>>;
type VecReceiver = mpsc::Receiver<Vec<u8>>;
type VecBroadcaster = broadcast::Sender<Arc<Vec<u8>>>;

pub struct Server;

impl Server {
    pub async fn start(
        output_rx: VecReceiver,
        timed_tx: VecSender,
        join_tx: JoinSender,
    ) -> Result<(), IrcError> {
        let listener = TcpListener::bind(SERVER_ADDR).await?;
        let (json_tx, mut json_rx) = broadcast::channel::<Arc<Vec<u8>>>(32);

        // consume all output from the initial broadcast recv handle
        // we don't have a better way to use it
        tokio::spawn(async move {
            while let Ok(_) = json_rx.recv().await {}
            Ok::<(), IrcError>(())
        });

        // parse irc output into json and broadcast
        start_json_sender(output_rx, json_tx.clone()).await;

        // handler for client connections
        tokio::spawn(async move {
            loop {
                let (sock, _) = listener.accept().await?;
                let (reader, mut writer) = sock.into_split();
                let mut json_rx = json_tx.subscribe();

                tokio::spawn(async move {
                    loop {
                        let output = match json_rx.recv().await {
                            Ok(o) => o,
                            Err(_) => break,
                        };
                        writer.write_all(&output).await?;
                    }
                    Ok::<(), IrcError>(())
                });

                start_client_json_receiver(reader, timed_tx.clone(), join_tx.clone()).await;
            }
            #[allow(unreachable_code)]
            Ok::<(), IrcError>(())
        });

        Ok(())
    }
}

async fn start_json_sender(mut output_rx: VecReceiver, json_tx: VecBroadcaster) {
    tokio::spawn(async move {
        // buffer in case we receive incomplete lines
        let mut rem = Vec::new();

        'outer: while let Some(mut vec) = output_rx.recv().await {
            let mut i = 0;
            let mut s = 0;

            if !rem.is_empty() {
                let mut tmp = Vec::new();
                tmp.append(&mut rem);
                tmp.append(&mut vec);
                vec = tmp;
            }

            while i < vec.len() {
                if vec[i] == b'\n' {
                    // optimize the common case of one complete line
                    if s == 0 && i + 1 == vec.len() {
                        json_tx.send(Arc::new(parse_irc_to_json(&mut vec)))?;
                        continue 'outer;
                    } else {
                        let mut slice = vec[s..i + 1].to_vec();
                        json_tx.send(Arc::new(parse_irc_to_json(&mut slice)))?;
                        s = i + 1;
                    }
                }
                i += 1;
            }

            // if we reach here, we may have a remainder
            if s < i {
                rem = Vec::from(&vec[s..i]);
            }
        }
        Ok::<(), IrcError>(())
    });
}

async fn start_client_json_receiver(
    mut reader: OwnedReadHalf,
    timed_tx: VecSender,
    join_tx: JoinSender,
) {
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        let mut rem = Vec::new();
        'outer: loop {
            let n = match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };

            let mut vec = (&buf[0..n]).to_vec();
            let mut i = 0;
            let mut s = 0;

            if !rem.is_empty() {
                let mut tmp = Vec::new();
                tmp.append(&mut rem);
                tmp.append(&mut vec);
                vec = tmp;
            }

            while i < vec.len() {
                if vec[i] == b'\n' {
                    // optimize the common case of one complete line
                    if s == 0 && i + 1 == vec.len() {
                        handle_json_command(&vec, &timed_tx, &join_tx).await?;
                        continue 'outer;
                    } else {
                        let slice = vec[s..i + 1].to_vec();
                        handle_json_command(&slice, &timed_tx, &join_tx).await?;
                        s = i + 1;
                    }
                }
                i += 1;
            }

            // if we reach here, we may have a remainder
            if s < i {
                rem = Vec::from(&vec[s..i]);
            }
        }
        Ok::<(), IrcError>(())
    });
}

fn parse_irc_to_json(line: &mut Vec<u8>) -> Vec<u8> {
    let mut obj = JsonValue::new_object();

    let mut start = 1; // skip leading control character
    let mut i = 1;
    let n = line.len();
    let mut equals = 0;

    while i < n {
        match line[i] {
            // allow escaping of control characters
            b'\\' => {
                i += 2;
                continue;
            }
            // end of metadata is signalled by an ascii space
            b' ' => break,
            b'=' => {
                equals = i;
            }
            // "key=value;" pairs
            b';' => {
                if equals != 0 {
                    let key = {
                        let key = &mut line[start..equals];

                        // convert dashes to underscores in keys
                        for ch in key.iter_mut() {
                            if *ch == b'-' {
                                *ch = b'_';
                            }
                        }

                        u8_to_str(&key)
                    };
                    let val = &line[equals + 1..i];

                    obj[key] = u8_to_str(val).into();
                }

                start = i + 1;
            }
            _ => {}
        }

        i += 1;
    }

    // :user!user@user.tmi.twitch.tv PRIVMSG #channel :words go here
    // find next 3 spaces           ^       ^        ^
    // but first, find the !
    let mut user_start = 0;
    while i < n {
        let c = line[i];

        match c {
            b':' => {
                user_start = i + 1;
            }
            b'!' => {
                obj["irc_account"] = u8_to_str(&line[user_start..i]).into();
                break;
            }
            _ => {}
        }

        i += 1;
    }

    let mut s_count = 0;
    let mut s_pos = [0, 0, 0];

    while i < n {
        let c = line[i];

        if c == b' ' {
            s_pos[s_count] = i;
            s_count += 1;
            if s_count == 3 {
                break;
            }
        }

        i += 1;
    }

    if s_count == 3 {
        obj["irc_msg_type"] = u8_to_str(&line[s_pos[0] + 1..s_pos[1]]).into();
        obj["irc_channel"] = u8_to_str(&line[s_pos[1] + 2..s_pos[2]]).into();

        let msg_start = s_pos[2] + 2;
        // emotes begin with "\0x01ACTION " and end with another \0x01 byte
        if msg_start < n {
            if line[msg_start] == b'\x01' {
                let emote_start = msg_start + 8;
                if emote_start < n {
                    obj["irc_emote"] = u8_to_str(&line[emote_start..n - 3]).into();
                }
            } else {
                obj["irc_msg"] = u8_to_str(&line[msg_start..n - 2]).into();
            }
        }
    }

    let mut ret = Vec::from(obj.dump());
    ret.push(b'\n');
    ret
}

async fn handle_json_command(
    v: &[u8],
    timed_tx: &VecSender,
    join_tx: &JoinSender,
) -> Result<(), IrcError> {
    // json-structured commands from the client
    let j = match json::parse(&u8_to_str(v)) {
        Ok(j) => j,
        Err(_) => return Ok(()),
    };

    let cmd = &j["json_meta_type"].to_string();

    match cmd.as_str() {
        "join" => {
            let s = &j["channel_name"];
            if s.is_string() {
                let chan = s.to_string();
                join_tx.send(JoinCmd::Join(chan.to_string())).await?;
            }
        }
        "leave" => {
            let s = &j["channel_name"];
            if s.is_string() {
                let chan = s.to_string();
                join_tx.send(JoinCmd::Leave(chan.to_string())).await?;
            }
        }
        "chat" => {
            let chan = {
                let s = &j["channel_name"];
                if s.is_string() {
                    s.to_string()
                } else {
                    return Ok(());
                }
            };
            let msg = {
                let s = &j["msg"];
                if s.is_string() {
                    s.to_string()
                } else {
                    return Ok(());
                }
            };

            let c = format!("PRIVMSG #{} :{}\r\n", chan, msg);
            timed_tx.send(c.as_bytes().to_vec()).await?;
        }
        _ => {}
    };
    Ok(())
}

fn u8_to_str(v: &[u8]) -> String {
    String::from_utf8_lossy(&v).into_owned()
}

#[tokio::test]
async fn test_split_irc_line() -> Result<(), IrcError> {
    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(32);
    let (json_tx, mut json_rx) = broadcast::channel::<Arc<Vec<u8>>>(32);

    start_json_sender(output_rx, json_tx.clone()).await;

    let part1 = b"@badge-info=subscriber/3;metadata=test; :user!user@user.tmi.twitch.tv PRIV";
    let part2 = b"MSG #channel :words go here\r\n";
    let json1 = "{\"badge_info\":\"subscriber/3\",\"metadata\":\"test\",\"irc_account\":\"user\",\"irc_msg_type\":\"PRIVMSG\",\"irc_channel\":\"channel\",\"irc_msg\":\"words go here\"}\n";

    output_tx.send(part1.to_vec()).await?;
    output_tx.send(part2.to_vec()).await?;

    let p1 = match json_rx.recv().await {
        Ok(p) => p,
        Err(_) => panic!("error reading broadcast value"),
    };

    assert_eq!(u8_to_str(&p1[0..p1.len()]), json1);

    Ok::<(), IrcError>(())
}

#[tokio::test]
async fn test_split_irc_line_multi_message() -> Result<(), IrcError> {
    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(32);
    let (json_tx, mut json_rx) = broadcast::channel::<Arc<Vec<u8>>>(32);

    start_json_sender(output_rx, json_tx.clone()).await;

    let part1 = b"@badge-info=subscriber/3;metadata=test; :user!user@user.tmi.twitch.tv PRIVMSG #channel :words go her";
    let part2 = b"e\r\n@badge-info=subscriber/1;metadata=test2; :user2!user2@user2.tmi.twitch.tv PRIVMSG #chan :example message\r\n";
    let json1 = "{\"badge_info\":\"subscriber/3\",\"metadata\":\"test\",\"irc_account\":\"user\",\"irc_msg_type\":\"PRIVMSG\",\"irc_channel\":\"channel\",\"irc_msg\":\"words go here\"}\n";
    let json2 = "{\"badge_info\":\"subscriber/1\",\"metadata\":\"test2\",\"irc_account\":\"user2\",\"irc_msg_type\":\"PRIVMSG\",\"irc_channel\":\"chan\",\"irc_msg\":\"example message\"}\n";

    output_tx.send(part1.to_vec()).await?;
    output_tx.send(part2.to_vec()).await?;

    let p1 = match json_rx.recv().await {
        Ok(p) => p,
        Err(_) => panic!("error reading broadcast value"),
    };

    assert_eq!(u8_to_str(&p1[0..p1.len()]), json1);

    let p2 = match json_rx.recv().await {
        Ok(p) => p,
        Err(_) => panic!("error reading broadcast value"),
    };

    assert_eq!(u8_to_str(&p2[0..p2.len()]), json2);

    Ok::<(), IrcError>(())
}
