use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use anyhow::{bail, Result};
use clap::ValueEnum;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};

const DEFAULT_CHANNEL_CAPACITY: usize = 128;

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
    async fn factory_switches_backends() {
        let memory = new_stream_transport(StreamTransportBackend::Memory);
        let pubsub = new_stream_transport(StreamTransportBackend::LocalPubsub);

        assert_eq!(memory.backend(), StreamTransportBackend::Memory);
        assert_eq!(pubsub.backend(), StreamTransportBackend::LocalPubsub);
    }
}
