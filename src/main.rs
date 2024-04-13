use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;

use std::{
    env,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let base_url = env::var("BASE_URL").context("BASE_URL is not set")?;
    let url = url::Url::parse(&format!("{}/top/send", base_url)).context("Failed to parse URL")?;

    let (stdin_tx, stdin_rx) = mpsc::channel(1);

    let sender = tokio::spawn(sender(url, stdin_rx));
    let producer = tokio::spawn(top(stdin_tx));

    sender.await.context("Failed to run sender")??;
    producer.await.context("Failed to run producer")??;

    Ok(())
}

async fn top(tx: Sender<String>) -> anyhow::Result<()> {
    let mut cmd = Command::new("top")
        .args(["-s", "10"])
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to spawn command")?;

    let stdout = cmd.stdout.as_mut().context("Failed to get stdout")?;
    let stdout_reader = BufReader::new(stdout);
    let stdout_lines = stdout_reader.lines();

    let mut is_filling = true;
    let mut has_sent = false;
    let mut lines = "".to_string();
    for line in stdout_lines {
        let line = line.context("Failed to read line")?;
        if line.starts_with("Processes:") {
            is_filling = true;
            has_sent = false;
            lines = line;
            continue;
        }
        if line.trim().is_empty() {
            is_filling = false;
        }

        if !is_filling && !has_sent {
            has_sent = true;
            tx.send(lines.clone())
                .await
                .context("Failed to send line")?;
        } else {
            lines.push_str(&format!("{}\n", line));
        }
    }

    cmd.wait().context("Failed to wait for command")?;

    return Ok(());
}

async fn sender(url: Url, mut rx: Receiver<String>) -> anyhow::Result<()> {
    let result = connect_async(url).await;
    let (ws_stream, _) = result?;
    println!("WebSocket handshake has been successfully completed");

    let (mut write, _) = ws_stream.split();

    while let Some(result) = rx.recv().await {
        if result.trim().is_empty() {
            continue;
        }
        write
            .send(Message::text(result))
            .await
            .context("Failed to send message")?;
    }
    Ok(())
}
