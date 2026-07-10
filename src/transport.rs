use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc, watch},
    task::JoinHandle,
};

const DEFAULT_CHANNEL_CAPACITY: usize = 128;
pub const NETWORK_STREAM_FRAME_VERSION: &str = "leash-stream-jsonl-v1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum StreamTransportBackend {
    Memory,
    #[default]
    LocalPubsub,
}

impl StreamTransportBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::LocalPubsub => "local-pubsub",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct StreamMessage {
    pub stream: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct NetworkStreamFrame {
    pub schema_version: String,
    pub stream: String,
    pub payload: Value,
}

impl NetworkStreamFrame {
    pub fn new(stream: impl Into<String>, payload: Value) -> Self {
        Self {
            schema_version: NETWORK_STREAM_FRAME_VERSION.to_string(),
            stream: stream.into(),
            payload,
        }
    }

    pub fn from_message(message: StreamMessage) -> Self {
        Self::new(message.stream, message.payload)
    }

    pub fn into_message(self) -> Result<StreamMessage> {
        self.validate()?;
        Ok(StreamMessage {
            stream: self.stream,
            payload: self.payload,
        })
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != NETWORK_STREAM_FRAME_VERSION {
            bail!(
                "unsupported network stream frame version '{}'",
                self.schema_version
            );
        }
        if self.stream.trim().is_empty() {
            bail!("network stream frame stream must not be empty");
        }
        Ok(())
    }
}

impl From<StreamMessage> for NetworkStreamFrame {
    fn from(message: StreamMessage) -> Self {
        Self::from_message(message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamRecvError {
    Closed,
    Lagged(u64),
}

pub trait StreamTransport: Send + Sync {
    fn backend(&self) -> StreamTransportBackend;
    fn publish(&self, stream: &str, payload: Value) -> Result<usize>;
    fn subscribe(&self, stream: &str) -> Result<StreamSubscriber>;
    fn shutdown(&self);
}

pub enum StreamSubscriber {
    Memory(mpsc::UnboundedReceiver<StreamMessage>),
    LocalPubsub(broadcast::Receiver<StreamMessage>),
}

impl StreamSubscriber {
    pub async fn recv(&mut self) -> std::result::Result<StreamMessage, StreamRecvError> {
        match self {
            Self::Memory(receiver) => receiver.recv().await.ok_or(StreamRecvError::Closed),
            Self::LocalPubsub(receiver) => match receiver.recv().await {
                Ok(message) => Ok(message),
                Err(broadcast::error::RecvError::Closed) => Err(StreamRecvError::Closed),
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    Err(StreamRecvError::Lagged(count))
                }
            },
        }
    }
}

pub fn new_stream_transport(backend: StreamTransportBackend) -> Arc<dyn StreamTransport> {
    match backend {
        StreamTransportBackend::Memory => Arc::new(MemoryTransport::default()),
        StreamTransportBackend::LocalPubsub => Arc::new(LocalPubsubTransport::default()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TcpJsonlStreamHubStatus {
    pub listen_addr: SocketAddr,
    pub accepted_peers: usize,
    pub active_peers: usize,
    pub published_messages: usize,
    pub rejected_frames: usize,
}

struct TcpJsonlStreamHubInner {
    listen_addr: SocketAddr,
    accepted_peers: AtomicUsize,
    active_peers: AtomicUsize,
    published_messages: AtomicUsize,
    rejected_frames: AtomicUsize,
}

impl TcpJsonlStreamHubInner {
    fn new(listen_addr: SocketAddr) -> Self {
        Self {
            listen_addr,
            accepted_peers: AtomicUsize::new(0),
            active_peers: AtomicUsize::new(0),
            published_messages: AtomicUsize::new(0),
            rejected_frames: AtomicUsize::new(0),
        }
    }

    fn status(&self) -> TcpJsonlStreamHubStatus {
        TcpJsonlStreamHubStatus {
            listen_addr: self.listen_addr,
            accepted_peers: self.accepted_peers.load(Ordering::SeqCst),
            active_peers: self.active_peers.load(Ordering::SeqCst),
            published_messages: self.published_messages.load(Ordering::SeqCst),
            rejected_frames: self.rejected_frames.load(Ordering::SeqCst),
        }
    }
}

pub struct TcpJsonlStreamHub {
    inner: Arc<TcpJsonlStreamHubInner>,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<Result<()>>,
}

impl TcpJsonlStreamHub {
    pub fn local_addr(&self) -> SocketAddr {
        self.inner.listen_addr
    }

    pub fn status(&self) -> TcpJsonlStreamHubStatus {
        self.inner.status()
    }

    pub async fn shutdown(self) -> Result<()> {
        let _ = self.shutdown.send(true);
        self.task.await.context("join TCP JSONL stream hub")??;
        Ok(())
    }
}

pub async fn spawn_tcp_jsonl_stream_hub(
    addr: SocketAddr,
    transport: Arc<dyn StreamTransport>,
) -> Result<TcpJsonlStreamHub> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind TCP JSONL stream hub at {addr}"))?;
    let listen_addr = listener.local_addr().context("read TCP JSONL hub addr")?;
    let inner = Arc::new(TcpJsonlStreamHubInner::new(listen_addr));
    let (shutdown, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(run_tcp_jsonl_stream_hub(
        listener,
        transport,
        inner.clone(),
        shutdown_rx,
    ));

    Ok(TcpJsonlStreamHub {
        inner,
        shutdown,
        task,
    })
}

async fn run_tcp_jsonl_stream_hub(
    listener: TcpListener,
    transport: Arc<dyn StreamTransport>,
    inner: Arc<TcpJsonlStreamHubInner>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                let (socket, _) = accepted.context("accept TCP JSONL stream hub peer")?;
                inner.accepted_peers.fetch_add(1, Ordering::SeqCst);
                inner.active_peers.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(handle_tcp_jsonl_stream_peer(
                    socket,
                    transport.clone(),
                    inner.clone(),
                ));
            }
        }
    }
    Ok(())
}

async fn handle_tcp_jsonl_stream_peer(
    socket: TcpStream,
    transport: Arc<dyn StreamTransport>,
    inner: Arc<TcpJsonlStreamHubInner>,
) {
    let mut reader = BufReader::new(socket);
    loop {
        match read_network_stream_message(&mut reader).await {
            Ok(Some(message)) => {
                let stream = message.stream;
                if transport.publish(&stream, message.payload).is_ok() {
                    inner.published_messages.fetch_add(1, Ordering::SeqCst);
                } else {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => {
                inner.rejected_frames.fetch_add(1, Ordering::SeqCst);
                break;
            }
        }
    }
    inner.active_peers.fetch_sub(1, Ordering::SeqCst);
}

pub async fn write_network_stream_frame<W>(writer: &mut W, frame: &NetworkStreamFrame) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    frame.validate()?;
    let line = serde_json::to_string(frame).context("serialize network stream frame")?;
    writer
        .write_all(line.as_bytes())
        .await
        .context("write network stream frame")?;
    writer
        .write_all(b"\n")
        .await
        .context("write network stream frame newline")?;
    writer.flush().await.context("flush network stream frame")?;
    Ok(())
}

pub async fn read_network_stream_frame<R>(reader: &mut R) -> Result<Option<NetworkStreamFrame>>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = String::new();
    let bytes = reader
        .read_line(&mut line)
        .await
        .context("read network stream frame")?;
    if bytes == 0 {
        return Ok(None);
    }
    let line = line.trim_end_matches(['\r', '\n']);
    let frame: NetworkStreamFrame =
        serde_json::from_str(line).context("parse network stream frame")?;
    frame.validate()?;
    Ok(Some(frame))
}

pub async fn write_network_stream_message<W>(writer: &mut W, message: &StreamMessage) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    write_network_stream_frame(
        writer,
        &NetworkStreamFrame::new(message.stream.clone(), message.payload.clone()),
    )
    .await
}

pub async fn read_network_stream_message<R>(reader: &mut R) -> Result<Option<StreamMessage>>
where
    R: AsyncBufRead + Unpin,
{
    read_network_stream_frame(reader)
        .await?
        .map(NetworkStreamFrame::into_message)
        .transpose()
}

pub async fn send_tcp_jsonl_stream_message(
    addr: SocketAddr,
    message: &StreamMessage,
) -> Result<()> {
    let mut socket = TcpStream::connect(addr)
        .await
        .with_context(|| format!("connect TCP JSONL stream peer at {addr}"))?;
    write_network_stream_message(&mut socket, message).await?;
    socket
        .shutdown()
        .await
        .context("shutdown TCP JSONL stream writer")?;
    Ok(())
}

pub async fn accept_tcp_jsonl_stream_message(listener: &TcpListener) -> Result<StreamMessage> {
    let (socket, _) = listener
        .accept()
        .await
        .context("accept TCP JSONL stream peer")?;
    let mut reader = BufReader::new(socket);
    read_network_stream_message(&mut reader)
        .await?
        .context("TCP JSONL peer closed before sending a stream message")
}

#[derive(Default)]
pub struct MemoryTransport {
    streams: Mutex<HashMap<String, Vec<mpsc::UnboundedSender<StreamMessage>>>>,
    closed: AtomicBool,
}

impl StreamTransport for MemoryTransport {
    fn backend(&self) -> StreamTransportBackend {
        StreamTransportBackend::Memory
    }

    fn publish(&self, stream: &str, payload: Value) -> Result<usize> {
        if self.closed.load(Ordering::SeqCst) {
            bail!("stream transport is shut down");
        }
        let mut streams = self.streams.lock();
        let Some(subscribers) = streams.get_mut(stream) else {
            return Ok(0);
        };
        let message = StreamMessage {
            stream: stream.to_string(),
            payload,
        };
        let mut delivered = 0;
        subscribers.retain(|subscriber| {
            if subscriber.send(message.clone()).is_ok() {
                delivered += 1;
                true
            } else {
                false
            }
        });
        Ok(delivered)
    }

    fn subscribe(&self, stream: &str) -> Result<StreamSubscriber> {
        if self.closed.load(Ordering::SeqCst) {
            bail!("stream transport is shut down");
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        self.streams
            .lock()
            .entry(stream.to_string())
            .or_default()
            .push(sender);
        Ok(StreamSubscriber::Memory(receiver))
    }

    fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.streams.lock().clear();
    }
}

pub struct LocalPubsubTransport {
    streams: Mutex<HashMap<String, broadcast::Sender<StreamMessage>>>,
    capacity: usize,
    closed: AtomicBool,
}

impl Default for LocalPubsubTransport {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CHANNEL_CAPACITY)
    }
}

impl LocalPubsubTransport {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
            capacity: capacity.max(1),
            closed: AtomicBool::new(false),
        }
    }

    fn sender(&self, stream: &str) -> broadcast::Sender<StreamMessage> {
        self.streams
            .lock()
            .entry(stream.to_string())
            .or_insert_with(|| broadcast::channel(self.capacity).0)
            .clone()
    }
}

impl StreamTransport for LocalPubsubTransport {
    fn backend(&self) -> StreamTransportBackend {
        StreamTransportBackend::LocalPubsub
    }

    fn publish(&self, stream: &str, payload: Value) -> Result<usize> {
        if self.closed.load(Ordering::SeqCst) {
            bail!("stream transport is shut down");
        }
        let sender = self.sender(stream);
        let message = StreamMessage {
            stream: stream.to_string(),
            payload,
        };
        Ok(sender.send(message).unwrap_or(0))
    }

    fn subscribe(&self, stream: &str) -> Result<StreamSubscriber> {
        if self.closed.load(Ordering::SeqCst) {
            bail!("stream transport is shut down");
        }
        Ok(StreamSubscriber::LocalPubsub(
            self.sender(stream).subscribe(),
        ))
    }

    fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.streams.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use tokio::{
        io::{AsyncWriteExt, BufReader},
        time::{sleep, timeout},
    };

    async fn recv_payload(receiver: &mut StreamSubscriber) -> Value {
        timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap()
            .payload
    }

    async fn wait_for_status<F>(hub: &TcpJsonlStreamHub, mut predicate: F)
    where
        F: FnMut(TcpJsonlStreamHubStatus) -> bool,
    {
        timeout(Duration::from_secs(2), async {
            loop {
                if predicate(hub.status()) {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn memory_transport_fans_out_to_subscribers() {
        let transport = MemoryTransport::default();
        let mut first = transport.subscribe("telemetry").unwrap();
        let mut second = transport.subscribe("telemetry").unwrap();

        assert_eq!(
            transport.publish("telemetry", json!({"seq": 1})).unwrap(),
            2
        );

        assert_eq!(first.recv().await.unwrap().payload, json!({"seq": 1}));
        assert_eq!(second.recv().await.unwrap().payload, json!({"seq": 1}));
    }

    #[tokio::test]
    async fn memory_transport_drops_unsubscribed_receivers() {
        let transport = MemoryTransport::default();
        let first = transport.subscribe("telemetry").unwrap();
        let mut second = transport.subscribe("telemetry").unwrap();
        drop(first);

        assert_eq!(
            transport.publish("telemetry", json!({"seq": 2})).unwrap(),
            1
        );
        assert_eq!(second.recv().await.unwrap().payload, json!({"seq": 2}));
    }

    #[tokio::test]
    async fn local_pubsub_reports_lagged_receiver() {
        let transport = LocalPubsubTransport::with_capacity(1);
        let mut receiver = transport.subscribe("telemetry").unwrap();

        transport.publish("telemetry", json!(1)).unwrap();
        transport.publish("telemetry", json!(2)).unwrap();
        transport.publish("telemetry", json!(3)).unwrap();

        assert_eq!(
            receiver.recv().await.unwrap_err(),
            StreamRecvError::Lagged(2)
        );
        assert_eq!(receiver.recv().await.unwrap().payload, json!(3));
    }

    #[tokio::test]
    async fn shutdown_closes_subscribers_and_refuses_new_messages() {
        let transport = MemoryTransport::default();
        let mut receiver = transport.subscribe("telemetry").unwrap();

        transport.shutdown();

        assert_eq!(receiver.recv().await.unwrap_err(), StreamRecvError::Closed);
        assert!(transport.publish("telemetry", json!({})).is_err());
        assert!(transport.subscribe("telemetry").is_err());
    }

    #[tokio::test]
    async fn network_stream_frame_round_trips_over_tcp_jsonl() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let expected = StreamMessage {
            stream: "telemetry".to_string(),
            payload: json!({"seq": 42, "ok": true}),
        };
        let server =
            tokio::spawn(async move { accept_tcp_jsonl_stream_message(&listener).await.unwrap() });

        send_tcp_jsonl_stream_message(addr, &expected)
            .await
            .unwrap();

        assert_eq!(server.await.unwrap(), expected);
    }

    #[tokio::test]
    async fn telemetry_contract_round_trips_over_tcp_jsonl_without_field_renaming() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let telemetry = crate::Harness::new(crate::HarnessConfig::default())
            .unwrap()
            .telemetry_stream_frame();
        let expected = StreamMessage {
            stream: "telemetry".to_string(),
            payload: serde_json::to_value(&telemetry).unwrap(),
        };
        let server =
            tokio::spawn(async move { accept_tcp_jsonl_stream_message(&listener).await.unwrap() });

        send_tcp_jsonl_stream_message(addr, &expected)
            .await
            .unwrap();
        let received = server.await.unwrap();
        let decoded: crate::TelemetryStreamFrame =
            serde_json::from_value(received.payload).unwrap();

        decoded.validate().unwrap();
        assert_eq!(
            decoded.telemetry.sensors.range_scan,
            telemetry.telemetry.sensors.range_scan
        );
        assert_eq!(
            decoded.telemetry.sensors.imu,
            telemetry.telemetry.sensors.imu
        );
        assert_eq!(
            decoded.telemetry.localization,
            telemetry.telemetry.localization
        );
        assert_eq!(
            decoded.visualization.localization,
            telemetry.visualization.localization
        );
    }

    #[tokio::test]
    async fn network_stream_frame_rejects_unsupported_versions() {
        let bytes =
            br#"{"schema_version":"old","stream":"telemetry","payload":{"seq":1}}"#.as_slice();
        let mut reader = BufReader::new(bytes);

        let err = read_network_stream_frame(&mut reader).await.unwrap_err();

        assert!(err
            .to_string()
            .contains("unsupported network stream frame version"));
    }

    #[tokio::test]
    async fn network_stream_frame_rejects_empty_stream_names() {
        let frame = NetworkStreamFrame::new("   ", json!({}));

        let err = frame.validate().unwrap_err();

        assert!(err
            .to_string()
            .contains("network stream frame stream must not be empty"));
    }

    #[tokio::test]
    async fn tcp_jsonl_stream_hub_publishes_multiple_messages_from_long_lived_peer() {
        let transport = new_stream_transport(StreamTransportBackend::Memory);
        let mut receiver = transport.subscribe("telemetry").unwrap();
        let hub = spawn_tcp_jsonl_stream_hub("127.0.0.1:0".parse().unwrap(), transport)
            .await
            .unwrap();
        let mut socket = TcpStream::connect(hub.local_addr()).await.unwrap();

        write_network_stream_message(
            &mut socket,
            &StreamMessage {
                stream: "telemetry".to_string(),
                payload: json!({"seq": 1}),
            },
        )
        .await
        .unwrap();
        write_network_stream_message(
            &mut socket,
            &StreamMessage {
                stream: "telemetry".to_string(),
                payload: json!({"seq": 2}),
            },
        )
        .await
        .unwrap();

        assert_eq!(recv_payload(&mut receiver).await, json!({"seq": 1}));
        assert_eq!(recv_payload(&mut receiver).await, json!({"seq": 2}));

        socket.shutdown().await.unwrap();
        wait_for_status(&hub, |status| {
            status.published_messages == 2 && status.active_peers == 0
        })
        .await;

        let status = hub.status();
        assert_eq!(status.accepted_peers, 1);
        assert_eq!(status.rejected_frames, 0);
        hub.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn tcp_jsonl_stream_hub_isolates_bad_peers() {
        let transport = new_stream_transport(StreamTransportBackend::Memory);
        let mut receiver = transport.subscribe("telemetry").unwrap();
        let hub = spawn_tcp_jsonl_stream_hub("127.0.0.1:0".parse().unwrap(), transport)
            .await
            .unwrap();

        let mut bad_peer = TcpStream::connect(hub.local_addr()).await.unwrap();
        bad_peer
            .write_all(br#"{"schema_version":"old","stream":"telemetry","payload":{"seq":0}}"#)
            .await
            .unwrap();
        bad_peer.write_all(b"\n").await.unwrap();
        bad_peer.shutdown().await.unwrap();
        wait_for_status(&hub, |status| status.rejected_frames == 1).await;

        send_tcp_jsonl_stream_message(
            hub.local_addr(),
            &StreamMessage {
                stream: "telemetry".to_string(),
                payload: json!({"seq": 3}),
            },
        )
        .await
        .unwrap();

        assert_eq!(recv_payload(&mut receiver).await, json!({"seq": 3}));
        wait_for_status(&hub, |status| {
            status.accepted_peers == 2 && status.active_peers == 0 && status.published_messages == 1
        })
        .await;

        let status = hub.status();
        assert_eq!(status.rejected_frames, 1);
        hub.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn factory_switches_backends() {
        let memory = new_stream_transport(StreamTransportBackend::Memory);
        let pubsub = new_stream_transport(StreamTransportBackend::LocalPubsub);

        assert_eq!(memory.backend(), StreamTransportBackend::Memory);
        assert_eq!(pubsub.backend(), StreamTransportBackend::LocalPubsub);
    }
}
