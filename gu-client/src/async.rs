#![allow(dead_code)]

use actix_web::{client, HttpMessage};
use error::Error;
use futures::{future, Future};
use gu_net::rpc::peer::PeerInfo;
use std::path::Path;
use std::str;
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize, Debug, Default, Builder)]
#[builder(pattern = "owned", setter(into))]
pub struct SessionInfo {
    name: String,
    environment: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, Builder)]
#[builder(pattern = "owned", setter(into))]
pub struct PeerSessionInfo {
    image: PeerSessionImage,
    environment: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct PeerSessionImage {
    url: String,
    hash: String,
}

/// Represents a connection to a single hub.
#[derive(Clone)]
pub struct Driver {
    driver_inner: Arc<DriverInner>,
}

struct DriverInner {
    url: String,
}

impl Driver {
    /// creates a driver from a given address:port, e.g. 127.0.0.1:61621
    pub fn from_addr<T>(addr: T) -> Driver
    where
        T: Into<String>,
    {
        Driver {
            driver_inner: Arc::new(DriverInner {
                url: format!("http://{}/", addr.into()),
            }),
        }
    }
    /// creates a new hub session
    pub fn new_session(
        &self,
        session_info_builder: SessionInfoBuilder,
    ) -> impl Future<Item = HubSession, Error = Error> {
        let sessions_url = format!("{}{}", self.driver_inner.url, "sessions");
        let session_info = match session_info_builder.build() {
            Ok(r) => r,
            _ => return future::Either::A(future::err(Error::InvalidHubSessionParameters)),
        };
        let request = match client::ClientRequest::post(sessions_url).json(session_info) {
            Ok(r) => r,
            _ => return future::Either::A(future::err(Error::CannotCreateRequest)),
        };
        let driver_for_session = self.clone();
        future::Either::B(
            request
                .send()
                .map_err(|_| Error::CannotSendRequest)
                .and_then(|response| response.body().map_err(|_| Error::CannotGetResponseBody))
                .and_then(|body| {
                    future::ok(HubSession {
                        driver: driver_for_session,
                        session_id: match str::from_utf8(&body.to_vec()) {
                            Ok(str) => str.to_string(),
                            _ => return future::err(Error::CannotGetResponseBody),
                        },
                    })
                }),
        )
    }
    pub fn auth_app<T, U>(&self, _app_name: T, _token: Option<U>)
    where
        T: Into<String>,
        U: Into<String>,
    {
    }
    /// returns all peers connected to the hub
    pub fn list_peers(&self) -> impl Future<Item = impl Iterator<Item = PeerInfo>, Error = Error> {
        let url = format!("{}{}", self.driver_inner.url, "peer");
        return match client::ClientRequest::get(url.clone()).finish() {
            Ok(r) => future::Either::A(
                r.send()
                    .map_err(|_| Error::CannotSendRequest)
                    .and_then(|response| response.json().map_err(|_| Error::InvalidJSONResponse))
                    .and_then(|answer_json: Vec<PeerInfo>| future::ok(answer_json.into_iter())),
            ),
            _ => future::Either::B(future::err(Error::CannotCreateRequest)),
        };
    }
}

/// Represents a hub session.
#[derive(Clone)]
pub struct HubSession {
    driver: Driver,
    pub session_id: String,
}

impl HubSession {
    /// adds peers to the hub
    pub fn add_peers<T, U>(&self, peers: T) -> impl Future<Item = (), Error = Error>
    where
        T: IntoIterator<Item = U>,
        U: AsRef<str>,
    {
        let add_url = format!(
            "{}{}/{}/peer",
            self.driver.driver_inner.url, "sessions", self.session_id
        );
        let peer_vec: Vec<String> = peers.into_iter().map(|peer| peer.as_ref().into()).collect();
        let request = match client::ClientRequest::post(add_url).json(peer_vec) {
            Ok(r) => r,
            _ => return future::Either::A(future::err(Error::CannotCreateRequest)),
        };
        future::Either::B(
            request
                .send()
                .map_err(|_| Error::CannotSendRequest)
                .and_then(|response| response.body().map_err(|_| Error::CannotGetResponseBody))
                .and_then(|body| {
                    println!("{:?}", body);
                    future::ok(())
                }),
        )
    }
    /// creates a new blob
    pub fn new_blob(&self) -> impl Future<Item = Blob, Error = Error> {
        let new_blob_url = format!(
            "{}{}/{}/blob",
            self.driver.driver_inner.url, "sessions", self.session_id
        );
        let request = match client::ClientRequest::post(new_blob_url).finish() {
            Ok(r) => r,
            _ => return future::Either::A(future::err(Error::CannotCreateRequest)),
        };
        let hub_session_copy = self.clone();
        future::Either::B(
            request
                .send()
                .map_err(|_| Error::CannotSendRequest)
                .and_then(|response| response.body().map_err(|_| Error::CannotGetResponseBody))
                .and_then(|body| {
                    println!("{:?}", body);
                    /* TODO test if blob_id is in the body of the answer */
                    future::ok(Blob {
                        hub_session: hub_session_copy,
                        blob_id: match str::from_utf8(&body.to_vec()) {
                            Ok(str) => str.to_string(),
                            _ => return future::err(Error::CannotGetResponseBody),
                        },
                    })
                }),
        )
    }
    /// gets a peer by its id
    pub fn peer<T>(&self, _peer_id: T)
    where
        T: Into<String>,
    {
        /* TODO */
    }
}

pub struct Blob {
    hub_session: HubSession,
    blob_id: String,
}

impl Blob {
    pub fn upload(&self, _path: &Path) {
        /* TODO PUT /sessions/{session-id}/blob/{blob-id} uploads blob */
    }
}

pub struct Peer {}

impl Peer {
    pub fn new_session(&self, _builder: PeerSessionInfoBuilder) -> PeerSession {
        /* TODO Adding peer HostDirect session to HUB session POST /sessions/{session-id}/peer/{node-id}/hd */
        PeerSession {}
    }
}

pub struct PeerSession {}
