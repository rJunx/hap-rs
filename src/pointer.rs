use futures::lock::Mutex;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

use crate::{accessory, event, storage};

pub type ControllerId = Arc<RwLock<Option<Uuid>>>;

pub type EventEmitter = Arc<Mutex<event::EventEmitter>>;

pub type EventSubscriptions = Arc<Mutex<Vec<(u64, u64)>>>;

pub type AccessoryDatabase = Arc<Mutex<storage::accessory_database::AccessoryDatabase>>;

pub type Accessory = Arc<Mutex<Box<dyn accessory::HapAccessory>>>;

pub type Storage = Arc<Mutex<Box<dyn storage::Storage>>>;

pub type Config = Arc<Mutex<crate::Config>>;

pub type MdnsResponder = Arc<Mutex<crate::transport::mdns::MdnsResponder>>;

/// FORK: the consumer-supplied source of camera stills for `POST /resource`.
///
/// A `std` lock rather than the async one used above: the provider is a plain synchronous
/// closure, so there is nothing to await and no lock held across a suspension point.
pub type SnapshotProvider = Arc<RwLock<Option<crate::transport::http::handler::resource::SnapshotFn>>>;
