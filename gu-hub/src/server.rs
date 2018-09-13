use actix::fut;
use actix::prelude::*;
use futures::prelude::*;

use gu_persist::config;

use actix_web;
use actix_web::server::StopServer;
use clap::{App, ArgMatches, SubCommand};
use gu_actix::*;
use std::borrow::Cow;
use std::net::ToSocketAddrs;
use std::sync::Arc;

use actix_web::error::JsonPayloadError;
use gu_base::{Decorator, Module};
use gu_lan::server;
use gu_p2p::rpc;
use gu_p2p::rpc::mock;
use gu_p2p::rpc::start_actor;
use gu_p2p::NodeId;
use gu_persist::config::ConfigManager;
use mdns::Responder;
use mdns::Service;
use serde::de;
use std::marker::PhantomData;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServerConfig {
    #[serde(default = "ServerConfig::default_p2p_port")]
    pub(crate) p2p_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    control_socket: Option<String>,
    #[serde(default = "ServerConfig::publish_service")]
    pub(crate) publish_service: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            p2p_port: Self::default_p2p_port(),
            control_socket: None,
            publish_service: Self::publish_service(),
        }
    }
}

impl ServerConfig {
    fn default_p2p_port() -> u16 {
        61622
    }
    fn publish_service() -> bool {
        true
    }

    pub fn p2p_addr(&self) -> impl ToSocketAddrs {
        ("0.0.0.0", self.p2p_port)
    }

    pub fn port(&self) -> u16 {
        self.p2p_port
    }
}

impl config::HasSectionId for ServerConfig {
    const SECTION_ID: &'static str = "server-cfg";
}

pub struct ServerModule {
    active: bool,
    config_path: Option<String>,
}

impl ServerModule {
    pub fn new() -> Self {
        ServerModule {
            config_path: None,
            active: false,
        }
    }
}

impl Module for ServerModule {
    fn args_declare<'a, 'b>(&self, app: App<'a, 'b>) -> App<'a, 'b> {
        app.subcommand(SubCommand::with_name("server").about("hub server management"))
    }

    fn args_consume(&mut self, matches: &ArgMatches) -> bool {
        let config_path = match matches.value_of("config-dir") {
            Some(v) => Some(v.to_string()),
            None => None,
        };

        if let Some(_m) = matches.subcommand_matches("server") {
            self.active = true;
            self.config_path = config_path.to_owned();
            return true;
        }
        false
    }

    fn run<D: Decorator + 'static + Sync + Send>(&self, decorator: D) {
        if !self.active {
            return;
        }

        let sys = actix::System::new("gu-hub");

        let _config = ServerConfigurer::new(decorator, self.config_path.clone()).start();

        let _ = sys.run();
    }
}

fn p2p_server<S>(_r: &actix_web::HttpRequest<S>) -> &'static str {
    "ok"
}

fn run_publisher(run: bool, port: u16) {
    if run {
        let responder = Responder::new().expect("Failed to run publisher");

        let svc = Box::new(responder.register(
            "_unlimited._tcp".to_owned(),
            "gu-hub".to_owned(),
            port,
            &["path=/", ""],
        ));

        let _svc: &'static mut Service = Box::leak(svc);
    }
}

fn prepare_lan_server(run: bool) {
    if run {
        // TODO: add it to endpoint
        start_actor(server::LanInfo());
    }
}

fn chat_route(
    req: &actix_web::HttpRequest<NodeId>,
) -> Result<actix_web::HttpResponse, actix_web::Error> {
    rpc::ws::route(req, req.state().clone())
}

pub(crate) struct ServerConfigurer<D: Decorator> {
    decorator: D,
    path: Option<String>,
}

impl<D: Decorator + 'static + Sync + Send> ServerConfigurer<D> {
    fn new(decorator: D, path: Option<String>) -> Self {
        Self { decorator, path }
    }

    pub fn config(&self) -> Addr<ConfigManager> {
        let config = config::ConfigManager::from_registry();
        println!("path={:?}", &self.path);

        if let Some(path) = &self.path {
            config.do_send(config::SetConfigPath::FsPath(Cow::Owned(path.clone())));
        }
        config
    }

    fn hub_configuration(&mut self, c: Arc<ServerConfig>, node_id: NodeId) -> Result<(), ()> {
        let decorator = self.decorator.clone();
        let server = actix_web::server::new(move || {
            decorator.decorate_webapp(
                actix_web::App::with_state(node_id.clone())
                    .handler("/p2p", p2p_server)
                    .scope("/m", mock::scope)
                    .resource("/ws/", |r| r.route().f(chat_route)),
            )
        });
        let _ = server.bind(c.p2p_addr()).unwrap().start();
        prepare_lan_server(c.publish_service);
        run_publisher(c.publish_service, c.p2p_port);

        Ok(())
    }
}

impl<D: Decorator + 'static> Actor for ServerConfigurer<D> {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut <Self as Actor>::Context) {
        use rand::*;

        let node_id: NodeId = thread_rng().gen();

        ctx.spawn(
            self.config()
                .send(config::GetConfig::new())
                .flatten_fut()
                .map_err(|e| println!("error ! {}", e))
                .into_actor(self)
                .and_then(move |config, act, ctx| {
                    act.hub_configuration(config, node_id);
                    fut::ok(ctx.stop())
                }),
        );
    }
}

impl<D: Decorator> Drop for ServerConfigurer<D> {
    fn drop(&mut self) {
        info!("server configured")
    }
}

#[derive(Fail, Debug)]
pub enum ClientError {
    #[fail(display = "MailboxError {}", _0)]
    Mailbox(#[cause] MailboxError),
    #[fail(display = "ActixError {}", _0)]
    ActixError(actix_web::Error),
    #[fail(display = "{}", _0)]
    SendRequestError(#[cause] actix_web::client::SendRequestError),
    #[fail(display = "{}", _0)]
    Json(#[cause] JsonPayloadError),
    #[fail(display = "config")]
    ConfigError,
}

impl From<actix_web::Error> for ClientError {
    fn from(e: actix_web::Error) -> Self {
        ClientError::ActixError(e)
    }
}

impl From<MailboxError> for ClientError {
    fn from(e : MailboxError) -> Self {
        ClientError::Mailbox(e)
    }
}

#[derive(Default)]
pub struct ServerClient {
    inner: (),
}

impl ServerClient {
    pub fn new() -> Self {
        ServerClient { inner: () }
    }

    pub fn get<T: de::DeserializeOwned + Send + 'static>(path: String) -> impl Future<Item= T, Error=ClientError> {
        ServerClient::from_registry().send(ResourceGet(path, PhantomData))
            .flatten_fut()
    }
}

impl Actor for ServerClient {
    type Context = Context<Self>;
}

impl Supervised for ServerClient {}
impl ArbiterService for ServerClient {}

struct ResourceGet<T>(String, PhantomData<T>);

impl<T: de::DeserializeOwned + 'static> Message for ResourceGet<T> {
    type Result = Result<T, ClientError>;
}

impl<T: de::DeserializeOwned + 'static> Handler<ResourceGet<T>> for ServerClient {
    type Result = ActorResponse<ServerClient, T, ClientError>;

    fn handle(&mut self, msg: ResourceGet<T>, ctx: &mut Self::Context) -> Self::Result {
        use actix_web::{client, HttpMessage};
        use futures::future;

        ActorResponse::async(
            ConfigManager::from_registry()
                .send(config::GetConfig::new())
                .flatten_fut()
                .map_err(|e| ClientError::ConfigError)
                .and_then(move |config: Arc<ServerConfig>| {
                    let url = format!("http://127.0.0.1:{}{}", config.port(), msg.0);
                    let client = match client::ClientRequest::get(url)
                        .header("Accept", "application/json")
                        .finish() {
                        Ok(cli) => cli,
                        Err(err) => return future::Either::B(future::err(err.into())),
                    };
                    future::Either::A(
                        client
                            .send()
                            .map_err(|e| ClientError::SendRequestError(e))
                            .and_then(|r| r.json::<T>().map_err(|e| ClientError::Json(e))),
                    )
                })
                .into_actor(self),
        )
    }
}