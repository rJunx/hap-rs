# Fork notes

A fork of [`ewilken/hap-rs`](https://github.com/ewilken/hap-rs), carried because upstream is
unmaintained (newest release is a pre-release, `0.1.0-pre.15`) **and does not build**.

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

`libmdns` publishes an A record for **every** interface. On a multi-homed host that is actively
harmful — a machine that also runs an access point advertises one hostname as two addresses:

```
accessory.local. → 10.0.0.20     LAN
accessory.local. → 192.168.8.1   access point, unroutable from the LAN
```

A controller picks one. When it picks the access-point address it hangs, and Home shows **"No
Response"** — with *nothing in the accessory's log*, because nothing ever connected. That absence
of evidence is what makes it look like a HAP-layer problem.

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

**`src/server/ip.rs`** — the defect; currently worked around in the consumer.

`configuration_number` is incremented **only** in `add_accessory` / `remove_accessory`, and only
for an `aid` not already in the aid cache. Once `aid=1` was cached, `c#` froze forever — no
matter how much the services and characteristics changed underneath.

HAP requires `c#` to increment whenever the accessory database changes; it is the **only** signal
a controller has that its cached `/accessories` is stale.

**The consequence was severe and invisible.** Every fix deployed during a long debugging session
left `c#=2`, so iOS had no reason to re-read and kept serving a cached copy of a database that
was, at the time, genuinely malformed. Fix after fix appeared to do nothing.

Worked around in the consumer rather than the fork, by hashing the serialised accessory and
bumping `c#` when the hash changes. The library should do this itself.

## 9. `pv=1.0`

**`src/config.rs`** — the default; currently overridden by the consumer.

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

## 14. Camera Operating Mode characteristics were `bool`, not `uint8`

**`src/characteristic/generated/{homekit_camera_active,event_snapshots_active,
periodic_snapshots_active,third_party_camera_active}.rs`**

HAP defines all four as `uint8` with `validValues: [0, 1]`. The code generator emitted
`Format::Bool`, so an accessory advertised `"format": "bool"` and sent `"value": true` where a
controller expects `1`.

Not cosmetic: these are how a camera tells a controller it is usable and that snapshots may be
taken, and a **home hub reads them before deciding to fetch one**.

Caught by a test comparing the serialised values against a working camera, before any controller
saw them.

## 15. An unknown controller was refused with the wrong error

**`src/transport/http/handler/pair_verify.rs`** — the most consequential fix in this file.

`load_pairing` returns `io::ErrorKind::NotFound` when a controller has no pairing on this
accessory, and `?` converted that through `From<io::Error>` into **`kTLVError_Unknown` (0x01)**,
documented in this crate as *"generic error to handle unexpected errors"*.

HAP's answer for an unrecognised controller is **`kTLVError_Authentication` (0x02)**. The
signature check twenty lines below already returned that for the *other* way verification can
fail; only the unknown-controller path was wrong.

The difference decides behaviour. `Unknown` means *something went wrong, try again*, so a
controller that is simply not paired retries forever. `Authentication` is a refusal, and a
controller that receives it falls back to credentials that do work.

**Observed:** an Apple home hub attempted pair-verify under its own identity every few seconds
for twelve hours, filling the log with `Io(NotFound)`, never falling back — so every request
routed through the hub failed, and the accessory showed "No Response" to anything outside the
home while working perfectly on the local network. With the correct error it switched to the
admin controller's identity on the next attempt and authenticated immediately.

## 16. `PUT /prepare` did not exist

**`src/transport/http/server.rs`**

HAP's timed-write preparation. A controller sends `{"ttl": <ms>, "pid": <n>}` and expects
`{"status": 0}`; the write that follows is executed only within that TTL.

The route was absent, so it 404'd — and an Apple **home hub issues it immediately after reading
`/accessories`**. On a 404 it abandons the connection and reconnects, indefinitely: roughly ten
full pair-verify cycles per second, none of which accomplish anything.

Responses mirror HAP-NodeJS `HAPServer.js:846-875`: empty body, missing `pid` or `ttl`, or any
method but PUT is `400` with `-70410` (`INVALID_VALUE_IN_REQUEST`); only a well-formed request
gets `0`.

> Note `pid` is a 64-bit identifier and real values exceed `i64::MAX` (`4953017753974871000` and
> `17325996466601334224` were both observed). Parsing it as a signed integer turns valid requests
> into rejections.

**Partial:** the TTL is acknowledged but not enforced — a later `PUT /characteristics` carrying a
`pid` is executed regardless of timing. That is the permissive direction: it accepts writes a
stricter accessory would reject, and never rejects a legitimate one. Enforcing it needs
per-connection state this router does not currently carry.

---

## Known-missing, not yet fixed

* **No media plane.** No SRTP, no RTP transmission, nothing that sends a frame. A consumer must
  supply its own on top of something like `webrtc-srtp`, and that is arguably where it belongs —
  a HAP library need not own an encoder pipeline.
* **No camera accessory type.** `accessory/defined/` stops at lightbulbs, locks and televisions,
  so a camera consumer has to supply its own.
* **The read path stalls if a single read delivers more than one frame.** Fixed for the common
  case (§ the `>=` change in `tcp.rs`), but `read_stream` still parses at most one frame per wake
  and relies on a later poll for the remainder. Not observed in practice.
* **`FileStorage::list_pairings` is path-fragile.** `list_files` returns absolute paths, and
  `read_bytes` pushes them onto the storage directory. `PathBuf::push` replaces on an absolute
  path, so it works — but only because the storage directory is absolute. A **relative** storage
  dir yields `data/data/pairings/x.json` and fails. Not hit when the storage directory is
  absolute, which is the normal deployment, so left alone rather than fixed speculatively.
