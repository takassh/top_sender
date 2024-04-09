use anyhow::Context;
use futures_util::{future, SinkExt, StreamExt, TryStreamExt};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{self, Receiver, Sender, UnboundedSender},
        Mutex,
    },
};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;

use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    net::SocketAddr,
    process::{Command, Stdio},
    sync::Arc,
};

type Tx = UnboundedSender<Message>;
type PeerMap = Arc<Mutex<HashMap<SocketAddr, Tx>>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = tokio::spawn(server());
    let client = tokio::spawn(client());

    let (_, _) = tokio::try_join!(server, client)?;

    Ok(())
}

async fn server() -> anyhow::Result<()> {
    let state = PeerMap::new(Mutex::new(HashMap::new()));

    let addr = "localhost:8080";
    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(addr).await;
    let listener = try_socket.context("Failed to bind")?;
    println!("Listening on: {}", addr);

    // Let's spawn the handling of each connection in a separate task.
    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(state.clone(), stream, addr));
    }

    Ok(())
}

async fn handle_connection(
    peer_map: PeerMap,
    raw_stream: TcpStream,
    addr: SocketAddr,
) -> anyhow::Result<()> {
    println!("Incoming TCP connection from: {}", addr);

    let ws_stream = tokio_tungstenite::accept_async(raw_stream)
        .await
        .context("Error during the websocket handshake occurred")?;
    println!("WebSocket connection established: {}", addr);

    // Insert the write part of this peer to the peer map.
    let (tx, _) = mpsc::unbounded_channel();
    peer_map.lock().await.insert(addr, tx);

    let (_, incoming) = ws_stream.split();

    let broadcast_incoming = incoming.try_for_each(|msg| {
        println!(
            "Received a message from {}: {}",
            addr,
            msg.to_text().unwrap()
        );

        future::ok(())
    });

    broadcast_incoming
        .await
        .context("Error during the websocket connection occurred")?;

    println!("{} disconnected", &addr);
    peer_map.lock().await.remove(&addr);

    Ok(())
}

async fn client() -> anyhow::Result<()> {
    let url = url::Url::parse("ws://localhost:8080").context("Failed to parse URL")?;

    let (stdin_tx, stdin_rx) = mpsc::channel(100);

    let producer = tokio::spawn(top(stdin_tx));
    let sender = tokio::spawn(sender(url, stdin_rx));

    producer.await.context("Failed to run producer")??;
    sender.await.context("Failed to run sender")??;

    Ok(())
}

async fn top(tx: Sender<String>) -> anyhow::Result<()> {
    let mut cmd = Command::new("top")
        .args(["-s", "5"])
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to spawn command")?;

    let stdout = cmd.stdout.as_mut().context("Failed to get stdout")?;
    let stdout_reader = BufReader::new(stdout);
    let stdout_lines = stdout_reader.lines();

    for line in stdout_lines {
        let line = line.context("Failed to read line")?;
        tx.send(line).await.context("Failed to send line")?;
    }

    cmd.wait().context("Failed to wait for command")?;

    return Ok(());
}

async fn sender(url: Url, mut rx: Receiver<String>) -> anyhow::Result<()> {
    println!("Hi, I'm the sender!");
    let (ws_stream, _) = connect_async(url).await.context("Failed to connect")?;
    println!("WebSocket handshake has been successfully completed");

    let (mut write, _) = ws_stream.split();

    loop {
        let Some(result) = rx.recv().await else {
            continue;
        };
        write
            .send(Message::text(result))
            .await
            .context("Failed to send message")?;
    }
}
