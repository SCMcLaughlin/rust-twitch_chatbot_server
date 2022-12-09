use crate::enums::*;
use crate::shared_stdout::SharedStdout;
use std::collections::HashSet;
use tokio::sync::mpsc;

pub struct ChannelList;

impl ChannelList {
    pub async fn start(
        timed_tx: mpsc::Sender<Vec<u8>>,
        sstdout: SharedStdout,
    ) -> Result<mpsc::Sender<JoinCmd>, IrcError> {
        let mut joined_channels = HashSet::new();
        let (join_tx, mut join_rx) = mpsc::channel::<JoinCmd>(32);

        tokio::spawn(async move {
            while let Some(j) = join_rx.recv().await {
                match j {
                    JoinCmd::JoinAll => {
                        let mut s = String::new();
                        for ch in &joined_channels {
                            s.push_str(&format!("join #{}\n", ch));
                            join(&timed_tx, &ch).await?;
                        }
                        if !s.is_empty() {
                            sstdout.write(s).await;
                        }
                    }
                    JoinCmd::Join(ch) => {
                        let ch = ch.to_ascii_lowercase();
                        if !joined_channels.contains(&ch) {
                            sstdout.write(format!("join #{}\n", ch)).await;
                            joined_channels.insert(ch.clone());
                            join(&timed_tx, &ch).await?;
                        }
                    }
                    JoinCmd::Leave(ch) => {
                        let ch = ch.to_ascii_lowercase();
                        if joined_channels.contains(&ch) {
                            sstdout.write(format!("leave #{}\n", ch)).await;
                            joined_channels.remove(&ch);
                            leave(&timed_tx, &ch).await?;
                        }
                    }
                }
            }
            Ok::<(), IrcError>(())
        });

        Ok(join_tx)
    }
}

async fn join_leave_impl(
    cmd: &str,
    irc_tx: &mpsc::Sender<Vec<u8>>,
    name: &String,
) -> Result<(), IrcError> {
    let cmd = format!("{} #{}\r\n", cmd, name)
        .to_string()
        .as_bytes()
        .to_vec();
    irc_tx.send(cmd).await?;
    Ok(())
}

async fn join(irc_tx: &mpsc::Sender<Vec<u8>>, name: &String) -> Result<(), IrcError> {
    join_leave_impl("JOIN", irc_tx, name).await
}

async fn leave(irc_tx: &mpsc::Sender<Vec<u8>>, name: &String) -> Result<(), IrcError> {
    join_leave_impl("PART", irc_tx, name).await
}

#[tokio::test]
async fn test_join_all() -> Result<(), IrcError> {
    use tokio::sync::mpsc::error::TryRecvError;

    let (timed_tx, mut timed_rx) = mpsc::channel::<Vec<u8>>(32);
    let sstdout = SharedStdout::new();

    let join_tx = ChannelList::start(timed_tx, sstdout).await?;

    join_tx.send(JoinCmd::Join("Example".to_string())).await?;
    join_tx.send(JoinCmd::Join("user".to_string())).await?;
    join_tx.send(JoinCmd::Join("CHAN123".to_string())).await?;

    let mut v = Vec::new();

    for _ in 0..3 {
        let s = match timed_rx.recv().await {
            Some(s) => s,
            None => panic!("error reading mpsc value"),
        };
        let s = match String::from_utf8(s) {
            Ok(s) => s,
            Err(_) => panic!("error converting to utf8"),
        };
        v.push(s);
    }

    assert_eq!(v[0], "JOIN #example\r\n");
    assert_eq!(v[1], "JOIN #user\r\n");
    assert_eq!(v[2], "JOIN #chan123\r\n");
    assert_eq!(Err(TryRecvError::Empty), timed_rx.try_recv());

    join_tx.send(JoinCmd::JoinAll).await?;

    let set1: HashSet<String> = v.into_iter().collect();
    let mut set2 = HashSet::new();

    for _ in 0..3 {
        let s = match timed_rx.recv().await {
            Some(s) => s,
            None => panic!("error reading mpsc value"),
        };
        let s = match String::from_utf8(s) {
            Ok(s) => s,
            Err(_) => panic!("error converting to utf8"),
        };
        set2.insert(s);
    }

    assert_eq!(set1, set2);
    assert_eq!(Err(TryRecvError::Empty), timed_rx.try_recv());

    Ok::<(), IrcError>(())
}
