use ed25519_dalek::Keypair as Ed25519Keypair;
//use eui48::MacAddress;
use macaddr::MacAddr6 as MacAddress;
use rand::{rngs::OsRng, Rng};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

use crate::{accessory::AccessoryCategory, BonjourFeatureFlag, BonjourStatusFlag, Pin};

/// The `Config` struct is used to store configuration options for the HomeKit Accessory Server.
///
/// # Examples
///
/// ```
/// use hap::{accessory::AccessoryCategory, Config, MacAddress, Pin};
///
/// let config = Config {
///     pin: Pin::new([1, 1, 1, 2, 2, 3, 3, 3]).unwrap(),
///     name: "Acme Lightbulb".into(),
///     device_id: MacAddress::from([10, 20, 30, 40, 50, 60]),
///     category: AccessoryCategory::Lightbulb,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// Socket IP address to serve on. Defaults to the IP of the system's first non-loopback network interface.
    pub host: IpAddr,
    /// Port to serve on. Defaults to `32000`.
    pub port: u16,
    /// 8 digit pin used for pairing. Defaults to `11122333`.
    ///
    /// The following pins are considered too easy and are therefore not allowed:
    /// - `12345678`
    /// - `87654321`
    /// - `00000000`
    /// - `11111111`
    /// - `22222222`
    /// - `33333333`
    /// - `44444444`
    /// - `55555555`
    /// - `66666666`
    /// - `77777777`
    /// - `88888888`
    /// - `99999999`
    pub pin: Pin,
    /// Model name of the accessory. E.g. "Acme Lightbulb".
    pub name: String,
    /// Device ID of the accessory. Generated randomly if not specified. This value is also used as the accessory's
    /// Pairing Identifier. Must be a unique random number generated at every factory reset and must persist across
    /// reboots.
    pub device_id: MacAddress, // Bonjour: id
    ///
    pub device_ed25519_keypair: Ed25519Keypair,
    /// Current configuration number. Is updated when an accessory, service, or characteristic is added or removed on
    /// the accessory server. Accessories must increment the config number after a firmware update.
    pub configuration_number: u64, // Bonjour: c#
    /// Current state number. This must have a value of `1`.
    pub state_number: u8, // Bonjour: s#
    /// Accessory category. Indicates the category that best describes the primary function of the accessory.
    pub category: AccessoryCategory, // Bonjour: ci
    /// Protocol version string `<major>.<minor>` (e.g. `"1.0"`). Defaults to `"1.0"` Required if value is not `"1.0"`.
    pub protocol_version: String, // Bonjour: pv
    /// Bonjour Status Flag. Defaults to `StatusFlag::NotPaired` and is changed to `StatusFlag::Zero` after a
    /// successful pairing.
    pub status_flag: BonjourStatusFlag, // Bonjour: sf
    /// Bonjour Feature Flag. Currently only used to indicate MFi compliance.
    pub feature_flag: BonjourFeatureFlag, // Bonjour: ff
    /// Optional maximum number of paired controllers.
    pub max_peers: Option<usize>,
    /// FORK: Setup ID -- four uppercase alphanumeric characters, e.g. `"7OSX"`.
    ///
    /// Pairs with [`Self::setup_hash`] and [`Self::setup_uri`]. iOS uses the hash to tell which
    /// discovered accessory a scanned or typed setup code belongs to; without it the Home app
    /// cannot bind the two together. Must persist across restarts, which it does -- `Config` is
    /// serialised into the pairing store.
    #[serde(default = "generate_setup_id")]
    pub setup_id: String, // Bonjour: sh (hashed)
}

impl Config {
    /// Redetermines the `host` field to the IP of the system's first non-loopback network interface.
    pub fn redetermine_local_ip(&mut self) { self.host = get_local_ip(); }

    /// Derives mDNS TXT records from the `Config`.
    pub(crate) fn txt_records(&self) -> [String; 9] {
        [
            format!("c#={}", self.configuration_number),
            format!("ff={}", self.feature_flag as u8),
            format!("id={}", self.device_id_string()),
            format!("md={}", self.name),
            format!("pv={}", self.protocol_version),
            format!("s#={}", self.state_number),
            format!("sf={}", self.status_flag as u8),
            format!("ci={}", self.category as u8),
            // FORK: was commented out as "still undocumented". It is documented, it is
            // `base64(SHA-512(setup_id || device_id)[..4])`, and leaving it out is why iOS
            // cannot match a typed setup code to this accessory.
            format!("sh={}", self.setup_hash()),
        ]
    }

    /// FORK: the device id exactly as it goes on the wire, in `id=` **and** in the setup hash.
    ///
    /// One function rather than two call sites, because iOS recomputes the hash from the `id=`
    /// it discovered: if these two ever disagreed by so much as letter case, pairing would fail
    /// with nothing to show why. (`macaddr` renders `{:02X}`, so this is uppercase today --
    /// which is what HAP wants -- but the point is that both sides move together.)
    pub fn device_id_string(&self) -> String {
        self.device_id.to_string()
    }

    /// FORK: the Bonjour `sh` value -- the first four bytes of `SHA-512(setup_id || device_id)`,
    /// base64-encoded.
    pub fn setup_hash(&self) -> String {
        use sha2::{Digest, Sha512};

        let mut hasher = Sha512::new();
        hasher.update(self.setup_id.as_bytes());
        hasher.update(self.device_id_string().as_bytes());

        base64::encode(&hasher.finalize()[..4])
    }

    /// FORK: the `X-HM://` setup URI, which is what a HomeKit setup QR code encodes.
    ///
    /// Layout, most significant bit first, packed into 64 bits and rendered as nine base36
    /// characters (zero-padded), then the four-character setup id:
    ///
    /// ```text
    ///   reserved (4) | category (8) | flags (4) | setup code (27)
    /// ```
    ///
    /// The flag set here is "supports IP" (bit 28 of the low word). The category's low bit is
    /// carried separately in bit 31 of the low word, which looks odd but matches the reference
    /// implementation byte for byte -- this value is parsed by iOS, so it follows HAP-NodeJS
    /// rather than an independent reading of the spec.
    pub fn setup_uri(&self) -> String {
        let code: u64 = self
            .pin
            .to_string()
            .chars()
            .filter(|c| c.is_ascii_digit())
            .fold(0u64, |acc, c| acc * 10 + u64::from(c as u8 - b'0'));

        let category = self.category as u64;

        let mut low = code | (1 << 28); // supports IP
        if category & 1 == 1 {
            low |= 1 << 31;
        }

        let value = ((category >> 1) << 32) | low;

        let mut encoded = to_base36(value);
        while encoded.len() < 9 {
            encoded.insert(0, '0');
        }

        format!("X-HM://{}{}", encoded, self.setup_id)
    }
}

/// FORK: uppercase base36, most significant digit first.
fn to_base36(mut value: u64) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

    if value == 0 {
        return "0".into();
    }

    let mut out = Vec::new();
    while value > 0 {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    out.reverse();

    String::from_utf8(out).expect("base36 digits are ASCII")
}

/// FORK: a random four-character uppercase-alphanumeric setup id.
fn generate_setup_id() -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

    // rand 0.7's two-argument `gen_range`, matching this crate's pin.
    let mut rng = OsRng {};
    (0..4)
        .map(|_| DIGITS[rng.gen_range(0, DIGITS.len())] as char)
        .collect()
}

impl Default for Config {
    fn default() -> Config {
        Config {
            host: get_local_ip(),
            port: 32000,
            pin: Pin::new([1, 1, 1, 2, 2, 3, 3, 3]).unwrap(),
            name: "Accessory".into(),
            device_id: generate_random_mac_address(),
            device_ed25519_keypair: generate_ed25519_keypair(),
            configuration_number: 1,
            state_number: 1,
            category: AccessoryCategory::Other,
            protocol_version: "1.0".into(),
            status_flag: BonjourStatusFlag::NotPaired,
            feature_flag: BonjourFeatureFlag::Zero,
            max_peers: None,
            setup_id: generate_setup_id(),
        }
    }
}

/// Generates a random MAC address.
fn generate_random_mac_address() -> MacAddress {
    let mut csprng = OsRng {};
    let eui = csprng.gen::<[u8; 6]>();
    MacAddress::from(eui)
}

/// Generates an Ed25519 keypair.
fn generate_ed25519_keypair() -> Ed25519Keypair {
    let mut csprng = OsRng {};
    Ed25519Keypair::generate(&mut csprng)
}

/// Returns the IP of the system's first non-loopback network interface or defaults to `127.0.0.1`.
///
/// FORK: was `get_if_addrs`, now `if_addrs`. The two crates each ship a `-sys` companion
/// declaring `links = "ifaddrs"`, and cargo permits only one package per `links` value -- so
/// depending on `get_if_addrs` while `libmdns` depends on `if-addrs` made this crate
/// **unresolvable**, in every published version and on upstream `main`. `if-addrs` is
/// `get_if_addrs`'s maintained successor and the call is identical.
fn get_local_ip() -> IpAddr {
    for iface in if_addrs::get_if_addrs().unwrap() {
        if !iface.is_loopback() {
            return iface.ip();
        }
    }
    "127.0.0.1".parse().unwrap()
}
