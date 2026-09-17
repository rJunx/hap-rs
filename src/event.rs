use futures::future::{join_all, BoxFuture};
use log::debug;
use serde_json::Value;
use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug)]
pub enum Event {
    ControllerPaired { id: Uuid },
    ControllerUnpaired { id: Uuid },
    CharacteristicValueChanged { aid: u64, iid: u64, value: Value },
}

type Listener = Box<dyn (Fn(&Event) -> BoxFuture<()>) + Send + Sync>;

struct Registered {
    /// Cleared when the listener's connection ends. See `add_scoped_listener`.
    alive: Arc<AtomicBool>,
    listener: Listener,
}

#[derive(Default)]
pub struct EventEmitter {
    listeners: Vec<Registered>,
}

impl EventEmitter {
    pub fn new() -> EventEmitter { EventEmitter { listeners: vec![] } }

    /// Register a listener that lives as long as the server does.
    pub fn add_listener(&mut self, listener: Listener) {
        self.listeners.push(Registered {
            alive: Arc::new(AtomicBool::new(true)),
            listener,
        });
    }

    /// FORK: register a listener that belongs to one connection.
    ///
    /// The transport adds a listener per accepted TCP connection and previously never removed
    /// one, so every connection ever made kept receiving events forever. That is not merely a
    /// leak: a listener holding a subscription on a **dead** socket still matched the
    /// characteristic and still consumed the event, so a controller that had gone away silently
    /// took the notification with it.
    ///
    /// Clearing `alive` when the connection ends both stops that and bounds the list.
    pub fn add_scoped_listener(&mut self, alive: Arc<AtomicBool>, listener: Listener) {
        // Pruned here rather than in `emit`, which takes `&self`. Connections are the only thing
        // that creates listeners, so cleaning up as one arrives keeps the list the size of the
        // live set without a sweeper task.
        self.listeners.retain(|r| r.alive.load(Ordering::Relaxed));

        self.listeners.push(Registered { alive, listener });
    }

    pub async fn emit(&self, event: &Event) {
        debug!("emitting event: {:?}", event);

        join_all(
            self.listeners
                .iter()
                .filter(|r| r.alive.load(Ordering::Relaxed))
                .map(|r| (r.listener)(&event)),
        )
        .await;
    }
}
