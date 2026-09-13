# Fork notes

A fork of [`ewilken/hap-rs`](https://github.com/ewilken/hap-rs), carried by
[`rs-hap-camera`](../rs-hap-camera) because upstream is unmaintained (newest release is a
pre-release, `0.1.0-pre.15`) **and does not build**.

Every change is marked `FORK:` in the source. This file is the index and the reasoning.

> **Why so many fixes?** `hap-rs` has never had a working camera accessory. It generates the
> camera *services* but populates none of them, ships no camera accessory type, and implements
> no media plane. Everything a camera touches — TLV8 values, the setup hash, the `/resource`
> route — is therefore code that has never executed. Expect more.

---

## 1. Unresolvable dependencies

**`Cargo.toml`, `src/config.rs`**

Neither the published crate nor upstream `main` can be selected by cargo at all:

| version | failure |
|---|---|
| `0.0.10` (what plain `cargo add hap` picks) | pulls `pnet` → `syntex`, deprecated ~2017, does not compile |
| `0.1.0-pre.15`, upstream `main` | depends on `get_if_addrs` while its own `libmdns` depends on `if-addrs`; both ship a `-sys` crate declaring `links = "ifaddrs"`, and cargo permits one package per `links` value |

Fixed by moving to `if-addrs 0.15`, whose modern releases are pure Rust and ship **no** `-sys`
crate — so `libmdns`'s `if-addrs-sys` becomes the only claimant of `ifaddrs`. One call site
(`get_local_ip`), identical API.

## 2. Setup hash (`sh`) was commented out

**`src/config.rs`**

The TXT record carried eight keys; the ninth, `sh`, was disabled with the note *"setup hash seems
to be still undocumented"*. It is documented: `base64(SHA-512(setup_id ‖ device_id)[..4])`.

Without it, iOS cannot bind a typed setup code to a discovered accessory, and manual entry falls
through to a generic path — in practice Apple's 11-digit **Matter** field, which can never accept
an 8-digit HomeKit code.

Added `Config::setup_id` (4 chars, persisted so it survives restarts), `setup_hash()`, and
`setup_uri()` — the `X-HM://` payload a HomeKit QR encodes. `device_id_string()` exists so the
`id=` record and the hash cannot drift apart: iOS recomputes the hash from the advertised `id`,
so a difference of letter case alone would break pairing with nothing to show why.

## 3. The ninth TXT record was silently dropped

**`src/transport/mdns.rs`**

`update_records` built a hand-written `[&tr[0], …, &tr[7]]`. Adding `sh` made the array nine
long; the literal stayed eight, and the ninth was discarded **without any error**. Now built from
the slice, so the two cannot diverge again.

Worth noting how this was found: the TXT record simply lacked `sh` after the change. The first
guess was mDNS caching — wrong, and checking the call site instead of theorising was what found it.

## 4. TLV8 values were never encoded for the wire

**`src/characteristic/mod.rs`**

Generated TLV8 characteristics are `Characteristic<Vec<u8>>`, and a `Vec<u8>` serialises to JSON
as `[1,2,3]`. HAP requires **base64 strings**. Both directions were wrong:

* **read** — `Serialize` now base64-encodes when `format == Tlv8`;
* **write** — `set_value` now base64-*decodes* a JSON string into the byte vector. A controller
  writing `SetupEndpoints` would otherwise be rejected with `InvalidValue(Tlv8)`.

Never exercised upstream because no shipped accessory populates a TLV8 characteristic.

## 5. `tlv::decode` panicked on malformed input — remotely, pre-authentication

**`src/tlv.rs`** — the most serious defect found.

`decode` indexed `tlv[p + 1]` and sliced `tlv[p + 2 .. p + 2 + l]` with **no bounds checks**, so
any TLV declaring a length past the end of the buffer aborted the process. `decode` parses
**unauthenticated pair-setup bodies**, which made four malformed bytes a remote crash of the
whole accessory — no pairing required.

Now bounds-checked, dropping a malformed tail rather than guessing at it. Four tests added
(`fork_tests`), including one that sweeps every length byte against every truncation.

Found by a test written as a formality, not by reading the code.

## 6. mDNS advertised every interface

**`src/transport/mdns.rs`**, `libmdns 0.6 → 0.10`

`libmdns` publishes an A record for **every** interface. On a multi-homed board that is actively
harmful — a BeagleBone running its own access point advertised one hostname as two addresses:

```
home-matter-bbbw.local. → 192.168.3.9     LAN
home-matter-bbbw.local. → 192.168.8.1     SoftAp0, unroutable from the LAN
```

A controller picks one. When it picked the access-point address it hung, and Home showed **"No
Response"** — with *nothing in the accessory's log*, because nothing ever connected. That absence
of evidence is what made it look like a HAP-layer problem for hours.

Fixed with `with_default_handle_and_ip_list`, added in `libmdns` 0.10, pinned to the configured
host address.

## 7. Unrouted requests 404'd in silence

**`src/transport/http/server.rs`**

An unknown route returned `404` with no log, which made "this crate does not implement that
endpoint" indistinguishable from "the controller went quiet". Now logged:

```
WARN no handler for POST /resource
```

That log immediately **disproved** a theory — iOS was never requesting `/resource` at all, so the
missing snapshot route was not the cause of anything, and no time was spent building it.

## 8. `c#` never changed when the accessory database changed

**`src/server/ip.rs` (the defect), worked around in `rs-hap-camera/src/main.rs`**

`configuration_number` is incremented **only** in `add_accessory` / `remove_accessory`, and only
for an `aid` not already in the aid cache. Once `aid=1` was cached, `c#` froze forever — no
matter how much the services and characteristics changed underneath.

HAP requires `c#` to increment whenever the accessory database changes; it is the **only** signal
a controller has that its cached `/accessories` is stale.

**The consequence was severe and invisible.** Every fix deployed during a long debugging session
left `c#=2`, so iOS had no reason to re-read and kept serving a cached copy of a database that
was, at the time, genuinely malformed. Fix after fix appeared to do nothing.

Worked around in the consumer rather than the fork: `rs-hap-camera` hashes the serialised
accessory and bumps `c#` when the hash changes. Upstream should do this itself.

## 9. `pv=1.0`

**`src/config.rs`** (default), overridden in `rs-hap-camera`.

`Config::default` advertises protocol version `1.0`. Every working HAP accessory observed on a
real LAN — two Homebridge instances, i.e. HAP-NodeJS — advertises **`1.1`**. `1.0` is the
protocol baseline and technically legal, but nothing modern publishes it.

## 10. TLV8 was not base64-encoded in `HapCharacteristic::get_value`

**`src/characteristic/mod.rs`**

The third call site of the same defect as §4, and the one that was missed. `Serialize` encodes for
`/accessories` and `set_value` decodes on the way in, but `get_value` returned `json!(value)` —
which for a `Characteristic<Vec<u8>>` is a JSON **array**, `[2,1,2]`.

That method feeds `GET /characteristics` *and* the write response, so the `SetupEndpoints` answer
was unreadable to a controller even once it was correctly computed and correctly returned. Fixing
it is what let iOS get past reading the accessory database.

## 11. A read fired the *update* callbacks

**`src/characteristic/mod.rs`**

`get_value` called `set_value`, which invokes `on_update`/`on_update_async`. A controller merely
**reading** a characteristic was therefore indistinguishable, to the accessory, from that
controller **writing** the value it had just read.

For a camera this is destructive rather than merely odd. The read callback returns the accessory's
own `SetupEndpoints` answer, whose TLV types (`0x01` session, `0x03` address, `0x04`/`0x05` SRTP
parameters) are the same ones a *request* uses — so it parsed as a perfectly valid new request and
replaced the negotiated session with one addressed to the accessory itself.

Reads now use `set_value_from_read`, which stores the value and emits change events but runs no
update callbacks.

## 12. No write-response support

**`src/transport/http/mod.rs`, `src/storage/accessory_database.rs`,
`src/transport/http/handler/characteristics.rs`**

HAP lets a controller request the new value **in the response to its own write**. `WriteObject`
named the field `remote`, so it deserialised from JSON `"remote"` and never matched the wire's
`r` (HAP-NodeJS reads `data.r`, `Accessory.js:1645`); `WriteResponseObject` had no `value` field at
all; and a successful write returned `204 No Content`, which discards any response body.

All three are fixed: `#[serde(rename = "r")]`, an optional `value`, and `200 OK` with the body
whenever any characteristic carries one.

> Worth recording honestly: this was implemented on the assumption that `SetupEndpoints` depends on
> it. **It does not** — iOS writes without `r` and reads the value back separately. The debug line
> added at the same time (`write to 1.14 did not request a response`) is what showed that. The
> support is correct per spec and harmless, but it was not the fix it was believed to be.

## 13. `POST /resource` did not exist

**`src/transport/http/handler/resource.rs` (new), `src/transport/http/server.rs`,
`src/server/ip.rs`, `src/pointer.rs`**

The still image every HomeKit camera serves. iOS asks for one as soon as it has read the accessory
database and draws the camera tile from the result; a 404 leaves it with no image, which presents
as **"No Response"** with pairing, `/accessories` and the stream configuration all correct.

Routed directly in `Api::call` rather than through `HandlerExt`, because it answers `image/jpeg` —
neither the JSON nor the TLV8 handler shape fits, and routing it through either would mean widening
a trait every handler implements. The consumer supplies frames via
`IpServer::set_snapshot_provider`.

---

## Known-missing, not yet fixed

* **No media plane.** No SRTP, no RTP transmission, nothing that sends a frame. `rs-hap-camera`
  implements its own (`h264.rs`, `sender.rs`) on top of `webrtc-srtp`, and that is arguably where
  it belongs — a HAP library need not own an encoder pipeline.
* **No camera accessory type.** `accessory/defined/` stops at lightbulbs, locks and televisions;
  `rs-hap-camera` supplies its own.
* **The read path stalls if a single read delivers more than one frame.** Fixed for the common
  case (§ the `>=` change in `tcp.rs`), but `read_stream` still parses at most one frame per wake
  and relies on a later poll for the remainder. Not observed in practice.
* **`FileStorage::list_pairings` is path-fragile.** `list_files` returns absolute paths, and
  `read_bytes` pushes them onto the storage directory. `PathBuf::push` replaces on an absolute
  path, so it works — but only because the storage directory is absolute. A **relative** storage
  dir yields `data/data/pairings/x.json` and fails. Not hit in production (the systemd unit uses
  `/var/lib/rs-hap-camera`), so left alone rather than fixed speculatively.
