//! Docker mode implementation

use super::envman;
use actix::prelude::*;
use async_docker::models::ContainerConfig;
use async_docker::{self, new_docker, DockerApi};
use deployment::DeployManager;
use deployment::Destroy;
use deployment::IntoDeployInfo;
use futures::future;
use futures::prelude::*;
use gu_model::dockerman::{CreateOptions, VolumeDef};
use gu_model::envman::*;
use gu_net::rpc::peer::PeerSessionInfo;
use gu_net::rpc::peer::PeerSessionStatus;
use gu_persist::config::ConfigModule;
use provision;
use std::borrow::Cow;
use std::collections::HashSet;
use workspace::Workspace;
use workspace::WorkspacesManager;

// Actor.
struct DockerMan {
    docker_api: Option<Box<DockerApi>>,
    deploys: DeployManager<DockerSession>,
    workspaces_man: WorkspacesManager,
}

impl Default for DockerMan {
    fn default() -> Self {
        let config = ConfigModule::new();
        DockerMan {
            docker_api: None,
            deploys: DeployManager::default(),
            workspaces_man: WorkspacesManager::new(&config, "docker").unwrap(),
        }
    }
}

struct DockerSession {
    workspace: Workspace,
    container: async_docker::communicate::Container,
    status: PeerSessionStatus,
}

impl DockerSession {
    fn do_open(&mut self) -> impl Future<Item = String, Error = String> {
        self.container.start().then(|r| match r {
            Ok(status) => Ok("OK".into()),
            Err(e) => Err(format!("{}", e)),
        })
    }

    fn do_close(&mut self) -> impl Future<Item = String, Error = String> {
        self.container
            .stop(None)
            .map_err(|e| format!("{}", e))
            .and_then(|v| Ok("OK".into()))
    }

    fn do_exec(
        &mut self,
        executable: String,
        mut args: Vec<String>,
    ) -> impl Future<Item = String, Error = String> {
        args.insert(0, executable);
        let cfg = {
            use async_docker::models::*;

            ExecConfig::new()
                .with_attach_stdout(true)
                .with_attach_stderr(true)
                .with_cmd(args)
        };

        self.container
            .exec(&cfg)
            .map_err(|e| format!("{}", e))
            .fold(String::new(), |mut s, (t, it)| {
                use std::str;

                match str::from_utf8(it.into_bytes().as_ref()) {
                    Ok(chunk_str) => s.push_str(chunk_str),
                    Err(_) => (),
                };

                Ok::<String, String>(s)
            })
    }

    fn do_download(
        &mut self,
        uri: String,
        file_path: String,
        format: ResourceFormat,
    ) -> impl Future<Item = String, Error = String> {
        use futures::sync::mpsc;
        use std::io;

        let stream = provision::download_stream(uri.as_str()).map(|x| {
            println!("{:?}", x);
            x
        });
        let opts = async_docker::build::ContainerArchivePutOptions::builder()
            .remote_path(file_path)
            .build();

        let (send, recv) = mpsc::channel(16);

        let recv_fut = self
            .container
            .archive_put_stream(
                &opts,
                recv.map_err(|()| io::Error::from(io::ErrorKind::Other)),
            )
            .into_future()
            .map_err(|e| e.to_string());

        let send_fut = send
            .sink_map_err(|e| e.to_string())
            .send_all(stream)
            .and_then(|(mut sink, _)| {
                println!("321");
                sink.close()
            });

        recv_fut.join(send_fut).map(|_| "OK".into())
    }

    fn do_upload(
        &mut self,
        uri: String,
        file_path: String,
        format: ResourceFormat,
    ) -> impl Future<Item = String, Error = String> {
        use actix_web::client;
        use std::io;

        let data = self
            .container
            .archive_get(file_path.as_str())
            .map_err(|e| e.to_string());

        let data: Box<Stream<Item = bytes::Bytes, Error = String>> = match format {
            ResourceFormat::Raw => Box::new(provision::untar_single_file_stream(data)),
            ResourceFormat::Tar => Box::new(data),
        };

        let data = data.map_err(|x| io::Error::from(io::ErrorKind::Other));

        future::result(client::put(uri.clone()).streaming(data))
            .map_err(|e| e.to_string())
            .and_then(|req| req.send().map_err(|e| e.to_string()))
            .and_then(move |res| {
                if res.status().is_success() {
                    Ok(format!("{:?} file uploaded", uri))
                } else {
                    Err(format!("Unsuccessful file upload: {}", res.status()))
                }
            })
    }
}

impl IntoDeployInfo for DockerSession {
    fn convert(&self, id: &String) -> PeerSessionInfo {
        PeerSessionInfo {
            id: id.clone(),
            name: self.workspace.name().to_string().clone(),
            status: self.status.clone(),
            tags: self.workspace.tags(),
            note: None,
            processes: HashSet::new(),
        }
    }
}

impl Destroy for DockerSession {}

impl DockerMan {
    fn container_config(
        image: String,
        host_config: async_docker::models::HostConfig,
    ) -> ContainerConfig {
        ContainerConfig::new()
            .with_image(image.into())
            .with_tty(true)
            .with_open_stdin(true)
            .with_attach_stdin(true)
            .with_attach_stderr(true)
            .with_attach_stdout(true)
            .with_volumes(
                [("/workspace".to_string(), json!({}))]
                    .to_vec()
                    .into_iter()
                    .collect(),
            )
            .with_host_config(host_config)
    }

    fn pull_config(uri: String) -> async_docker::build::PullOptions {
        async_docker::build::PullOptions::builder()
            .image(uri)
            .build()
    }

    fn binds_and_workspace(&self, msg: &CreateSession<CreateOptions>) -> (Vec<String>, Workspace) {
        let mut workspace = self.workspaces_man.workspace();
        let binds = msg
            .options
            .volumes
            .iter()
            .filter_map(|vol: &VolumeDef| {
                vol.source_dir()
                    .and_then(|s| vol.target_dir().map(|t| (s, t)))
                    .map(|(s, t)| {
                        workspace.add_volume(vol.clone());
                        format!("{}:{}", s, t)
                    })
            })
            .collect();

        (binds, workspace)
    }
}

impl Actor for DockerMan {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut <Self as Actor>::Context) {
        match new_docker(None) {
            Ok(docker_api) => {
                self.docker_api = Some(docker_api);
                envman::register("docker", ctx.address())
            }
            Err(e) => {
                error!("docker start failed: {}", e);
                ctx.stop()
            }
        }
    }
}

impl envman::EnvManService for DockerMan {
    type CreateOptions = CreateOptions;
}

impl Handler<CreateSession<CreateOptions>> for DockerMan {
    type Result = ActorResponse<DockerMan, String, Error>;

    fn handle(
        &mut self,
        msg: CreateSession<CreateOptions>,
        _ctx: &mut Self::Context,
    ) -> <Self as Handler<CreateSession<CreateOptions>>>::Result {
        debug!("create session for: {}", &msg.image.uri);

        match self.docker_api {
            Some(ref api) => {
                let Image { uri, hash } = msg.image.clone();

                let (binds, workspace) = self.binds_and_workspace(&msg);

                workspace
                    .create_dirs()
                    .expect("Creating session dirs failed");
                let host_config = async_docker::models::HostConfig::new().with_binds(binds);

                let opts = Self::container_config(uri.clone(), host_config);
                info!("config: {:?}", &opts);

                let pull_image_fut = api.images().pull(&Self::pull_config(uri));
                let create_container_fut = api.containers().create(&opts);

                let pull_and_create = pull_image_fut
                    .for_each(|x| Ok(debug!("{:?}", x)))
                    .and_then(|_| create_container_fut)
                    .map(|c| c.id().to_owned())
                    .map_err(|e| Error::IoError(format!("{}", e)));

                ActorResponse::async(fut::wrap_future(pull_and_create).and_then(
                    move |id, act: &mut DockerMan, _| {
                        if let Some(ref api) = act.docker_api {
                            let deploy = DockerSession {
                                workspace,
                                container: api.container(Cow::from(id.clone())),
                                status: PeerSessionStatus::CREATED,
                            };
                            act.deploys.insert_deploy(id.clone(), deploy);
                            fut::ok(id)
                        } else {
                            fut::err(Error::UnknownEnv(msg.env_type.clone()))
                        }
                    },
                ))
            }
            None => ActorResponse::reply(Err(Error::UnknownEnv(msg.env_type))),
        }
    }
}

impl DockerMan {
    fn run_for_deployment<F, R>(
        &mut self,
        deployment_id: String,
        f: F,
    ) -> Box<ActorFuture<Actor = DockerMan, Item = String, Error = String>>
    where
        F: FnOnce(&mut DockerSession) -> R,
        R: Future<Item = String, Error = String> + 'static,
    {
        let deployment = match self.deploys.deploy_mut(&deployment_id) {
            Ok(deployment) => deployment,
            Err(e) => return Box::new(fut::err(format!("{}", e))),
        };

        Box::new(fut::wrap_future(f(deployment)))
    }
}

fn run_command(
    docker_man: &mut DockerMan,
    session_id: String,
    command: Command,
) -> Box<ActorFuture<Actor = DockerMan, Item = String, Error = String>> {
    if docker_man.docker_api.is_none() {
        return Box::new(fut::err("Docker API not initialized properly".to_string()));
    }

    match command {
        Command::Open => docker_man.run_for_deployment(session_id, DockerSession::do_open),
        Command::Close => docker_man.run_for_deployment(session_id, DockerSession::do_close),
        Command::Exec { executable, args } => docker_man
            .run_for_deployment(session_id, |deployment| {
                deployment.do_exec(executable, args)
            }),
        Command::Start { executable, args } => Box::new(fut::ok("Start mock".to_string())),
        Command::Stop { child_id } => Box::new(fut::ok("Stop mock".to_string())),
        Command::DownloadFile {
            uri,
            file_path,
            format,
        } => docker_man.run_for_deployment(session_id, |deployment| {
            deployment.do_download(uri, file_path, format)
        }),
        Command::UploadFile {
            uri,
            file_path,
            format,
        } => docker_man.run_for_deployment(session_id, |deployment| {
            deployment.do_upload(uri, file_path, format)
        }),
        Command::AddTags(tags) => Box::new(fut::result(
            docker_man
                .deploys
                .deploy_mut(&session_id)
                .map(|session| {
                    session.workspace.add_tags(tags);
                    format!(
                        "tags inserted. Current tags are: {:?}",
                        &session.workspace.tags()
                    )
                })
                .map_err(|e| e.to_string()),
        )),
        Command::DelTags(tags) => Box::new(fut::result(
            docker_man
                .deploys
                .deploy_mut(&session_id)
                .map(|session| {
                    session.workspace.remove_tags(tags);
                    format!(
                        "tags removed. Current tags are: {:?}",
                        &session.workspace.tags()
                    )
                })
                .map_err(|e| e.to_string()),
        )),
    }
}

fn run_commands(
    hd_man: &mut DockerMan,
    session_id: String,
    commands: Vec<Command>,
) -> impl ActorFuture<Actor = DockerMan, Item = Vec<String>, Error = Vec<String>> {
    let f: Box<dyn ActorFuture<Actor = DockerMan, Item = Vec<String>, Error = Vec<String>>> =
        Box::new(future::ok(Vec::new()).into_actor(hd_man));

    commands.into_iter().fold(f, |acc, command| {
        let session_id = session_id.clone();
        Box::new(acc.and_then(|mut vec, act, _ctx| {
            run_command(act, session_id, command).then(move |i, _, _| match i {
                Ok(a) => {
                    vec.push(a);
                    fut::ok(vec)
                }
                Err(a) => {
                    vec.push(a);
                    fut::err(vec)
                }
            })
        }))
    })
}

impl Handler<SessionUpdate> for DockerMan {
    type Result = ActorResponse<DockerMan, Vec<String>, Vec<String>>;

    fn handle(&mut self, msg: SessionUpdate, _ctx: &mut Self::Context) -> Self::Result {
        if !self.deploys.contains_deploy(&msg.session_id) {
            return ActorResponse::reply(Err(
                vec![Error::NoSuchSession(msg.session_id).to_string()],
            ));
        }
        let session_id = msg.session_id.clone();

        ActorResponse::async(run_commands(self, session_id, msg.commands))
    }
}

impl Handler<GetSessions> for DockerMan {
    type Result = ActorResponse<DockerMan, Vec<PeerSessionInfo>, ()>;

    fn handle(
        &mut self,
        _msg: GetSessions,
        _ctx: &mut Self::Context,
    ) -> <Self as Handler<GetSessions>>::Result {
        ActorResponse::reply(Ok(self.deploys.deploys_info()))
    }
}

impl Handler<DestroySession> for DockerMan {
    type Result = ActorResponse<DockerMan, String, Error>;

    fn handle(
        &mut self,
        msg: DestroySession,
        _ctx: &mut Self::Context,
    ) -> <Self as Handler<DestroySession>>::Result {
        let container_id = msg.session_id.into();

        let api = match self.docker_api {
            Some(ref api) => api,
            _ => return ActorResponse::reply(Err(Error::UnknownEnv("docker".into()))),
        };

        ActorResponse::async(
            api.container(container_id)
                .delete()
                .map_err(|_e| Error::Error("docker error".into()))
                .and_then(|_| Ok("done".into()))
                .into_actor(self),
        )
    }
}

struct Init;

impl gu_base::Module for Init {
    fn run<D: gu_base::Decorator + Clone + 'static>(&self, _decorator: D) {
        gu_base::run_once(|| {
            let _ = DockerMan::default().start();
        });
    }
}

pub fn module() -> impl gu_base::Module {
    Init
}
