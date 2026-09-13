use libmdns::{Responder, Service};
use log::debug;

use crate::pointer;

/// An mDNS Responder. Used to announce the Accessory's name and HAP TXT records to potential controllers.
pub struct MdnsResponder {
    config: pointer::Config,
    responder: Responder,
    service: Option<Service>,
    task: Option<Box<dyn futures::Future<Output = ()> + Unpin + std::marker::Send>>,
}

impl MdnsResponder {
    /// Creates a new mDNS Responder.
    ///
    /// FORK: restricted to the configured host address.
    ///
    /// `libmdns` otherwise publishes an A record for **every** interface, which is actively
    /// harmful on a multi-homed host: a machine that also runs an access point advertises both
    /// its LAN address and the access-point address under one hostname. A controller picks one,
    /// and when it picks an address unroutable from its own network it hangs and the accessory
    /// shows as "No Response" -- with nothing in the log, because nothing ever connects.
    pub async fn new(config: pointer::Config) -> Self {
        let host = config.lock().await.host;

        let (responder, task) = libmdns::Responder::with_default_handle_and_ip_list(vec![host])
            .expect("creating mDNS responder");

        MdnsResponder {
            config,
            responder,
            service: None,
            task: Some(task),
        }
    }

    /// Derives new mDNS TXT records from the server's `Config`.
    pub async fn update_records(&mut self) {
        debug!("attempting to set mDNS records");

        self.service = None;

        let c = self.config.lock().await;

        let name = c.name.clone();
        let port = c.port;
        let tr = c.txt_records();

        drop(c);

        // FORK: was a hand-written `[&tr[0], ..., &tr[7]]`, which published only the first
        // EIGHT records. Adding `sh` to `txt_records` made it nine, and the ninth was dropped
        // silently -- the array grew, the literal did not, and nothing complained. Built from
        // the slice now, so the two cannot drift apart again.
        let txt: Vec<&str> = tr.iter().map(String::as_str).collect();

        self.service = Some(
            self.responder
                .register("_hap._tcp".into(), name.as_str(), port, &txt),
        );

        debug!("setting mDNS records: {:?}", &tr);
    }

    /// Returns the mDNS task to throw on a scheduler.
    pub fn run_handle(&mut self) -> Box<dyn futures::Future<Output = ()> + Unpin + std::marker::Send> {
        match self.task.take() {
            Some(task) => task,
            // if the task handle is gone, recreate the whole responder
            None => {
                let (responder, task) = libmdns::Responder::with_default_handle().expect("creating mDNS responder");
                self.responder = responder;

                task
            },
        }
    }
}
