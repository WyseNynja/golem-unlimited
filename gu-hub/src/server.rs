use actix::fut;
use actix::prelude::*;
use futures::future;
use futures::prelude::*;
use tokio;

use gu_persist::config;

use actix_web::server::HttpServer;
use actix_web::server::StopServer;
use actix_web::{self, *};
use clap::{self, ArgMatches, SubCommand};
use gu_actix::*;
use std::borrow::Cow;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use tokio_uds::UnixListener;

use gu_p2p::rpc;
use mdns::Responder;
use gu_p2p::NodeId;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerConfig {
    #[serde(default = "ServerConfig::default_p2p_port")]
    p2p_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    control_socket: Option<String>,
    #[serde(default = "ServerConfig::publish_service")]
    publish_service: bool,
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
}

impl ServerConfig {
    fn p2p_addr(&self) -> impl ToSocketAddrs {
        ("0.0.0.0", self.p2p_port)
    }
}

impl config::HasSectionId for ServerConfig {
    const SECTION_ID: &'static str = "server-cfg";
}

pub fn clap_declare<'a, 'b>() -> clap::App<'a, 'b> {
    SubCommand::with_name("server")
}

pub fn clap_match(m: &ArgMatches) {
    let config_path = match m.value_of("config-dir") {
        Some(v) => Some(v.to_string()),
        None => None,
    };

    if let Some(m) = m.subcommand_matches("server") {
        println!("server");
        run_server(config_path.to_owned());
    }
}

fn run_server(config_path: Option<String>) {
    use actix;

    let sys = actix::System::new("gu-hub");

    let config = ServerConfigurer(None, config_path).start();

    let _ = sys.run();

}

fn p2p_server<S>(r: &HttpRequest<S>) -> &'static str {
    "ok"
}

fn run_publisher(run: bool, port: u16) {
    if run {
        let responder = Responder::new().expect("Failed to run publisher");

        let _svc = responder.register(
            "_unlimited._tcp".to_owned(),
            "gu-hub".to_owned(),
            port,
            &["path=/", ""],
        );
    }
}

struct ServerConfigurer(Option<Recipient<StopServer>>, Option<String>);

fn chat_route(req: &HttpRequest<NodeId>) -> Result<HttpResponse, actix_web::Error> {
    rpc::ws::route(req, req.state().clone())
}


impl Actor for ServerConfigurer {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut <Self as Actor>::Context) {
        let config = config::ConfigManager::from_registry();

        println!("path={:?}", &self.1);
        if let Some(path) = &self.1 {
            config.do_send(config::SetConfigPath::FsPath(Cow::Owned(path.clone())));
        }

        use rand::*;

        let node_id : NodeId = thread_rng().gen();

        ctx.spawn(
            config
                .send(config::GetConfig::new())
                .flatten_fut()
                .map_err(|e| println!("error ! {}", e))
                .and_then(move |c: Arc<ServerConfig>| {
                    let server = server::new(
                        move || App::with_state(node_id.clone())
                            .handler("/p2p", p2p_server)
                            .scope("/m", rpc::mock::scope)
                            .resource("/ws/", |r| r.route().f(chat_route))
                    );
                    let _ = server.bind(c.p2p_addr()).unwrap().start();
                    run_publisher(c.publish_service, c.p2p_port);

                    Ok(())
                })
                .into_actor(self)
                .and_then(|_, _, ctx| fut::ok(ctx.stop())),
        );
    }
}

impl Drop for ServerConfigurer {
    fn drop(&mut self) {
        info!("server configured")
    }
}
