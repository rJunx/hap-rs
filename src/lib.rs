pub use ed25519_dalek::Keypair as Ed25519Keypair;
pub use futures;
pub use macaddr::MacAddr6 as MacAddress;
pub use serde_json;

pub use crate::{
    config::Config,
    error::Error,
    hap_type::HapType,
    pin::Pin,
    transport::bonjour::{BonjourFeatureFlag, BonjourStatusFlag},
};

mod config;
mod error;
mod event;
mod hap_type;
mod pin;
mod pointer;
mod transport;

// FORK: was `mod tlv;`. A camera accessory must build the TLV8 payloads for
// `SupportedVideoStreamConfiguration` and its siblings itself -- this crate defines the camera
// service but leaves every one of those characteristics empty -- and there is no reason to
// carry a second TLV8 encoder just to do that.
pub mod tlv;

/// Definitions of HomeKit accessories.
pub mod accessory;
/// Definitions of HomeKit characteristics.
pub mod characteristic;
/// Representation of paired controllers.
pub mod pairing;
/// The HomeKit Accessory Server implementation.
pub mod server;
/// Definitions of HomeKit services.
pub mod service;
/// Representations of persistent storage.
pub mod storage;

/// `Result` type redefinition.
pub type Result<T> = std::result::Result<T, Error>;
