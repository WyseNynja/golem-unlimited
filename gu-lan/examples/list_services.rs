extern crate log;
extern crate actix;
extern crate gu_lan;
extern crate env_logger;
extern crate futures;

use actix::prelude::*;
use env_logger::Builder;
use log::LevelFilter;
use futures::future;
use futures::Future;

fn main() {
    Builder::from_default_env()
        .filter_level(LevelFilter::Debug)
        .init();

    let sys = actix::System::new("none_example");
    let actor = gu_lan::resolve_actor::ResolveActor::new();
    let address = actor.start();
    let res = address.send(gu_lan::service::Service::new("gu-provider", "_http._tcp"));

    Arbiter::spawn(res.then(|res| {
        match res {
            Ok(result) => println!("Received result: {:?}", result),
            _ => println!("Something went wrong"),
        }

        future::result(Ok(()))
    }));

    let _ = sys.run();
}
