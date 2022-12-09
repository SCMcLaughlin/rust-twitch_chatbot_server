use tokio::io::{self, AsyncWriteExt};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct SharedStdout {
    stdout_tx: mpsc::Sender<String>,
}

impl SharedStdout {
    pub fn new() -> Self {
        let (stdout_tx, mut stdout_rx) = mpsc::channel::<String>(64);

        // stdout thread
        tokio::spawn(async move {
            let mut stdout = io::stdout();
            while let Some(msg) = stdout_rx.recv().await {
                _ = stdout.write_all(msg.as_bytes()).await;
            }
        });

        Self {
            stdout_tx: stdout_tx,
        }
    }

    pub async fn write(&self, msg: String) {
        _ = self.stdout_tx.send(msg).await;
    }

    pub async fn write_str(&self, msg: &str) {
        self.write(String::from(msg)).await;
    }
}
