use futures::{
    channel::oneshot,
    future::{self, BoxFuture, Future, FutureExt, TryFutureExt},
    lock::Mutex,
};
use hyper::{server::conn::Http, service::Service, Body, Method, Request, Response, StatusCode};
use log::{debug, error, info, warn};
use std::{
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::net::TcpListener;

use crate::{
    event::Event,
    pointer,
    transport::{
        http::{
            event_response,
            handler::{
                accessories::Accessories,
                characteristics::{GetCharacteristics, UpdateCharacteristics},
                identify::Identify,
                pair_setup::PairSetup,
                pair_verify::PairVerify,
                pairings::Pairings,
                resource,
                HandlerExt,
                JsonHandler,
                TlvHandler,
            },
            status_response,
            EventObject,
        },
        tcp::{EncryptedStream, Session, StreamWrapper},
    },
    Error,
    Result,
};

struct Handlers {
    pub pair_setup: Arc<Mutex<Box<dyn HandlerExt + Send + Sync>>>,
    pub pair_verify: Arc<Mutex<Box<dyn HandlerExt + Send + Sync>>>,
    pub accessories: Arc<Mutex<Box<dyn HandlerExt + Send + Sync>>>,
    pub get_characteristics: Arc<Mutex<Box<dyn HandlerExt + Send + Sync>>>,
    pub put_characteristics: Arc<Mutex<Box<dyn HandlerExt + Send + Sync>>>,
    pub pairings: Arc<Mutex<Box<dyn HandlerExt + Send + Sync>>>,
    pub identify: Arc<Mutex<Box<dyn HandlerExt + Send + Sync>>>,
}

struct Api {
    controller_id: pointer::ControllerId,
    event_subscriptions: pointer::EventSubscriptions,
    config: pointer::Config,
    storage: pointer::Storage,
    accessory_database: pointer::AccessoryDatabase,
    event_emitter: pointer::EventEmitter,
    // FORK: supplies stills for `POST /resource`.
    snapshot: pointer::SnapshotProvider,
    handlers: Handlers,
}

impl Api {
    fn new(
        controller_id: pointer::ControllerId,
        event_subscriptions: pointer::EventSubscriptions,
        config: pointer::Config,
        storage: pointer::Storage,
        accessory_database: pointer::AccessoryDatabase,
        event_emitter: pointer::EventEmitter,
        snapshot: pointer::SnapshotProvider,
        session_sender: oneshot::Sender<Session>,
    ) -> Self {
        Api {
            controller_id,
            event_subscriptions,
            config,
            storage,
            accessory_database,
            event_emitter,
            snapshot,
            handlers: Handlers {
                pair_setup: Arc::new(Mutex::new(Box::new(TlvHandler::from(PairSetup::new())))),
                pair_verify: Arc::new(Mutex::new(Box::new(TlvHandler::from(PairVerify::new(session_sender))))),
                accessories: Arc::new(Mutex::new(Box::new(JsonHandler::from(Accessories::new())))),
                get_characteristics: Arc::new(Mutex::new(Box::new(JsonHandler::from(GetCharacteristics::new())))),
                put_characteristics: Arc::new(Mutex::new(Box::new(JsonHandler::from(UpdateCharacteristics::new())))),
                pairings: Arc::new(Mutex::new(Box::new(TlvHandler::from(Pairings::new())))),
                identify: Arc::new(Mutex::new(Box::new(JsonHandler::from(Identify::new())))),
            },
        }
    }
}

impl Service<Request<Body>> for Api {
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = std::result::Result<Self::Response, Self::Error>> + Send>>;
    type Response = Response<Body>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let (parts, body) = req.into_parts();
        let method = parts.method;
        let uri = parts.uri;

        // FORK: kept for the unrouted-request log below, which needs the method after the match.
        let logged_method = method.clone();
        let logged_path = uri.path().to_string();

        let is_snapshot_request = logged_method == Method::POST && logged_path == "/resource";

        let mut handler: Option<Arc<Mutex<Box<dyn HandlerExt + Send + Sync>>>> = match (method, uri.path()) {
            (Method::POST, "/pair-setup") => Some(self.handlers.pair_setup.clone()),
            (Method::POST, "/pair-verify") => Some(self.handlers.pair_verify.clone()),
            (Method::GET, "/accessories") => Some(self.handlers.accessories.clone()),
            (Method::GET, "/characteristics") => Some(self.handlers.get_characteristics.clone()),
            (Method::PUT, "/characteristics") => Some(self.handlers.put_characteristics.clone()),
            (Method::POST, "/pairings") => Some(self.handlers.pairings.clone()),
            (Method::POST, "/identify") => Some(self.handlers.identify.clone()),
            _ => None,
        };

        let controller_id = self.controller_id.clone();
        let event_subscriptions = self.event_subscriptions.clone();
        let config = self.config.clone();
        let storage = self.storage.clone();
        let accessory_database = self.accessory_database.clone();
        let event_emitter = self.event_emitter.clone();
        let snapshot = self.snapshot.clone();

        // FORK: `POST /resource` is handled here rather than through `HandlerExt`, because it
        // answers with `image/jpeg` -- neither the JSON nor the TLV8 handler shape fits, and
        // routing it through either would mean widening a trait every handler implements.
        if is_snapshot_request {
            return async move {
                let request = resource::parse(body).await;
                let (width, height) = request.dimensions();

                let image = {
                    let provider = snapshot.read().expect("reading the snapshot provider");
                    match provider.as_ref() {
                        Some(provider) => provider(width, height),
                        None => {
                            warn!("a controller asked for a camera snapshot, but no provider is registered");
                            return status_response(StatusCode::NOT_FOUND);
                        },
                    }
                };

                match image {
                    Ok(image) => {
                        debug!("serving a {}x{} snapshot, {} Bytes", width, height, image.len());
                        resource::image_response(image)
                    },
                    Err(e) => {
                        error!("could not produce a camera snapshot: {:?}", e);
                        status_response(StatusCode::INTERNAL_SERVER_ERROR)
                    },
                }
            }
            .boxed();
        }

        let fut = async move {
            match handler.take() {
                Some(handler) =>
                    handler
                        .lock()
                        .await
                        .handle(
                            uri,
                            body,
                            controller_id,
                            event_subscriptions,
                            config,
                            storage,
                            accessory_database,
                            event_emitter,
                        )
                        .await,
                // FORK: log what we refused.
                //
                // An unrouted request used to 404 in silence, which made a missing route
                // indistinguishable from a controller that simply went quiet. `POST /resource`
                // -- the camera snapshot every HomeKit camera serves, and which this crate does
                // not implement -- lands here.
                None => {
                    log::warn!("no handler for {logged_method} {logged_path}");
                    future::ready(status_response(StatusCode::NOT_FOUND)).await
                },
            }
        }
        .boxed();

        fut
    }
}

#[derive(Clone)]
pub struct Server {
    config: pointer::Config,
    storage: pointer::Storage,
    accessory_database: pointer::AccessoryDatabase,
    event_emitter: pointer::EventEmitter,
    mdns_responder: pointer::MdnsResponder,
    // FORK: shared with `IpServer`, so a provider registered after construction is still seen.
    snapshot: pointer::SnapshotProvider,
}

impl Server {
    pub fn new(
        config: pointer::Config,
        storage: pointer::Storage,
        accessory_database: pointer::AccessoryDatabase,
        event_emitter: pointer::EventEmitter,
        mdns_responder: pointer::MdnsResponder,
        snapshot: pointer::SnapshotProvider,
    ) -> Self {
        Server {
            config,
            storage,
            accessory_database,
            event_emitter,
            mdns_responder,
            snapshot,
        }
    }

    pub fn run_handle(&self) -> BoxFuture<Result<()>> {
        let config = self.config.clone();
        let storage = self.storage.clone();
        let accessory_database = self.accessory_database.clone();
        let event_emitter = self.event_emitter.clone();
        let mdns_responder = self.mdns_responder.clone();
        let snapshot = self.snapshot.clone();

        async move {
            let config_lock = config.lock().await;
            let socket_addr = SocketAddr::new(config_lock.host, config_lock.port);
            drop(config_lock);

            info!("binding TCP listener on {}", &socket_addr);
            let listener = TcpListener::bind(socket_addr).await?;

            mdns_responder.lock().await.update_records().await;

            loop {
                let (stream, _socket_addr) = listener.accept().await?;

                debug!("incoming TCP stream from {}", stream.peer_addr()?);

                let (
                    encrypted_stream,
                    stream_incoming,
                    stream_outgoing,
                    session_sender,
                    incoming_waker,
                    outgoing_waker,
                ) = EncryptedStream::new(stream);
                let stream_wrapper =
                    StreamWrapper::new(stream_incoming, stream_outgoing.clone(), incoming_waker, outgoing_waker);
                let event_subscriptions = Arc::new(Mutex::new(vec![]));

                let api = Api::new(
                    encrypted_stream.controller_id.clone(),
                    event_subscriptions.clone(),
                    config.clone(),
                    storage.clone(),
                    accessory_database.clone(),
                    event_emitter.clone(),
                    snapshot.clone(),
                    session_sender,
                );

                event_emitter.lock().await.add_listener(Box::new(move |event| {
                    let event_subscriptions_ = event_subscriptions.clone();
                    let stream_outgoing_ = stream_outgoing.clone();
                    async move {
                        match *event {
                            Event::CharacteristicValueChanged { aid, iid, ref value } => {
                                let mut dropped_subscriptions = vec![];
                                for (i, &(s_aid, s_iid)) in event_subscriptions_.lock().await.iter().enumerate() {
                                    if s_aid == aid && s_iid == iid {
                                        let event = EventObject {
                                            aid,
                                            iid,
                                            value: value.clone(),
                                        };
                                        let event_res =
                                            event_response(vec![event]).expect("couldn't create event response");
                                        if stream_outgoing_.unbounded_send(event_res).is_err() {
                                            dropped_subscriptions.push(i);
                                        }
                                    }
                                }
                                let mut ev = event_subscriptions_.lock().await;
                                for s in dropped_subscriptions {
                                    ev.remove(s);
                                }
                            },
                            _ => {},
                        }
                    }
                    .boxed()
                }));

                let mut http = Http::new();
                http.http1_only(true);
                http.http1_half_close(true);
                http.http1_keep_alive(true);
                http.http1_preserve_header_case(true);

                tokio::spawn(encrypted_stream.map_err(|e| error!("{:?}", e)).map(|_| ()));
                tokio::spawn(
                    http.serve_connection(stream_wrapper, api)
                        .map_err(|e| error!("{:?}", e))
                        .map(|_| ()),
                );
            }

            #[allow(unreachable_code)]
            Ok(())
        }
        .boxed()
    }
}
