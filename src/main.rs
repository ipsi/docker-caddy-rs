use docker_api::models::{ContainerInspect200Response, EventMessage};
use docker_api::opts::{ContainerListOpts, ContainerFilter, ExecCreateOpts, ExecStartOpts};
use docker_api::{conn::TtyChunk, Docker, opts::EventsOpts};
use tokio_stream::StreamExt;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::str;
use std::sync::OnceLock;
use indoc::indoc;
use tracing_subscriber;
use tracing::{info, warn, debug, error};
use clap::Parser;

/// Watch docker for Container events, write those out to a set of Caddy snippets, then
/// trigger a reload of both Caddy instances.
///
/// There are two Caddy instances due a quirk with Docker networking, where it will not
/// retain the source IP address, making it impossible to have Caddy (or the Auth server)
/// deny access based on IP ranges.
///
/// The only workaround to this on macOS is having two Caddy instances - running outside Docker,
/// and one inside, with the first handling SSL termination and then delegating to the second,
/// which actually performs the reverse-proxying to the applications.
///
/// On Linux, there are other workarounds, such as modifying the network rules, or running Docker
/// in "Host" networking mode, etc.
#[derive(Debug, Parser)]
#[command(name="docker-caddyfile-updater", bin_name="docker-caddyfile-updater")]
struct Cli {
    /// Path to the "local" Caddy binary, which handles SSL termination and proxies to the Docker
    /// Caddy instance.
    #[arg(long, visible_alias="lcbp", env, default_value = "/usr/local/bin/caddy")]
    local_caddy_bin_path: PathBuf,
    /// Path to the "local" Caddy configuration directory, used to set the working directory when
    /// reloading Caddy
    #[arg(long, visible_alias="lccd", env, default_value = "/usr/local/etc")]
    local_caddy_config_dir: PathBuf,
    /// Directory to write the "local" snippets out to (Caddy will then import these)
    #[arg(long, visible_alias="lcsd", env)]
    local_caddy_snippets_dir: PathBuf,
    /// Is the "local" Caddy actually running on docker rather than the host? Could be the case if
    /// the "local" Caddy is using Host networking, for example.
    #[arg(long, visible_alias="lcod", env)]
    local_caddy_on_docker: bool,
    /// Path to the Caddy binary inside the Docker file (defaults to just "caddy" as it's on the
    /// path).
    #[arg(long, visible_alias="dcbp", env, default_value = "caddy")]
    docker_caddy_bin_path: PathBuf,
    /// Path of the Caddy configuration directory inside Docker. Only used to set the working
    /// directory when reloading Caddy
    #[arg(long, visible_alias="dccd", env, default_value = "/etc/caddy")]
    docker_caddy_config_dir: PathBuf,
    /// Directory to write the snippets for the second Caddy instance. This should be a directory
    /// that is on the host machine and is mounted into Docker.
    #[arg(long, visible_alias="dcsd", env)]
    docker_caddy_snippets_dir: PathBuf,
    /// The prefix for the labels used to determine what should and should not be exposed via
    /// Caddy. e.g., "my.name"
    /// Available labels are:
    /// * app - the name of the application, prepended to the domain or local domain
    /// * port - the port the app runs on (mandatory, no default)
    /// * external - if the app will be exposed via the domain_name (true), or the local domain
    /// (otherwise)
    /// * auth (oidc, headers, none) - if headers, include the "auth-headers" snippet, otherwise do
    /// nothing.
    #[arg(long, visible_alias="lp", env)]
    label_prefix: String,
    /// Prefix for the local domain, used by the generated Caddy snippets for anything where
    /// "external" is false or absent.
    #[arg(long, visible_alias="ldp", env)]
    local_domain_prefix: String,
    /// The general domain name, e.g., example.com
    #[arg(long, visible_alias="dn", env)]
    domain_name: String,
    /// Path to the docker.sock file, used to communicate with the Docker API
    #[arg(long, visible_alias="dsp", env, default_value="/var/run/docker.sock")]
    docker_socket_path: PathBuf,
}

struct Config {
    app_name_label: String,
    port_label: String,
    external_label: String,
    auth_label: String,
    external_domain: String,
    local_domain: String,
    local_caddy: CaddyConfig,
    docker_caddy: CaddyConfig,
    docker_config: DockerConfig,
}

struct CaddyConfig {
    bin_path: PathBuf,
    config_dir: PathBuf,
    snippets_dir: PathBuf,
    location: CaddyLocation,
}

enum CaddyLocation {
    Local,
    Docker(String),
}

struct DockerConfig {
    docker_socket_path: PathBuf,
}

impl Config {
    fn new(args: Cli) -> Self {
        Self {
            app_name_label: format!("{}.app", &args.label_prefix),
            port_label: format!("{}.port", &args.label_prefix),
            external_label: format!("{}.external", &args.label_prefix),
            auth_label: format!("{}.auth", &args.label_prefix),
            local_domain: format!("{}.{}", &args.local_domain_prefix, &args.domain_name),
            external_domain: args.domain_name,
            local_caddy: CaddyConfig {
                bin_path: args.local_caddy_bin_path,
                config_dir: args.local_caddy_config_dir,
                snippets_dir: args.local_caddy_snippets_dir,
                location: CaddyLocation::Local, 
            },
            docker_caddy: CaddyConfig {
                bin_path: args.docker_caddy_bin_path,
                config_dir: args.docker_caddy_config_dir,
                snippets_dir: args.docker_caddy_snippets_dir,
                location: CaddyLocation::Docker("caddy".to_string()),
            },
            docker_config: DockerConfig {
                docker_socket_path: args.docker_socket_path
            },
        }
    }
}

fn config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| { Config::new(Cli::parse()) })
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type ApplicationData = HashMap<String, AppData>;

#[cfg(unix)]
pub fn new_docker() -> Result<Docker> {
    Ok(Docker::unix(&config().docker_config.docker_socket_path))
}

#[cfg(not(unix))]
use Result as DockerResult;

#[cfg(not(unix))]
pub fn new_docker() -> DockerResult<Docker> {
    Docker::new("tcp://127.0.0.1:8080")
}

pub fn print_chunk(chunk: TtyChunk) {
    match chunk {
        TtyChunk::StdOut(bytes) => {
            println!("Stdout: {}", str::from_utf8(&bytes).unwrap_or_default())
        }
        TtyChunk::StdErr(bytes) => {
            eprintln!("Stdout: {}", str::from_utf8(&bytes).unwrap_or_default())
        }
        TtyChunk::StdIn(_) => unreachable!(),
    }
}

#[derive(Debug)]
struct ContainerSummaryInternal {
    id: String,
    container_name: String,
    labels: Option<HashMap<String, String>>,
}

impl ContainerSummaryInternal {
    fn new_from_inspect(container: &ContainerInspect200Response) -> Result<Self> {
        let container_name = container.name.as_ref().map(|s| s.as_str()).map(|s| s.strip_prefix("/").unwrap_or(s).to_string()).unwrap();
        Ok(ContainerSummaryInternal {
            id: container.id.clone().unwrap(),
            container_name,
            labels: container.config.as_ref().unwrap().labels.clone(),
        })
    }
}

#[derive(Debug)]
struct EventSummaryInternal {
    id: String,
    app_name: Option<String>,
    container_name: String,
    old_name: Option<String>
}

impl EventSummaryInternal {
    fn new_from_event(event: &EventMessage) -> Result<Self> {
        Ok(EventSummaryInternal {
            id: event.actor.as_ref().unwrap().id.clone().unwrap(),
            app_name: event.actor.as_ref().unwrap().attributes.as_ref().unwrap().get(&config().app_name_label).map(|s| s.to_string()).clone(),
            container_name: event.actor.as_ref().unwrap().attributes.as_ref().unwrap().get("name").map(|s| s.strip_prefix("/").unwrap_or(s).to_string()).unwrap(),
            old_name: event.actor.as_ref().unwrap().attributes.as_ref().unwrap().get("oldName").map(|s| s.strip_prefix("/").unwrap_or(s).to_string()),
        })
    }
}

#[derive(Debug)]
enum CaddyAuthType {
    Oidc,
    TrustedHeaders,
    Unknown(String),
    None,
}

#[derive(Debug)]
struct AppData {
    app_name: String,
    containers: Vec<AppContainerData>,
    port: u16,
    external: bool,
    auth_type: CaddyAuthType,
}

impl AppData {
    fn name_from_summary(summary: &ContainerSummaryInternal) -> Option<String> {
        summary
            .labels
            .as_ref()
            .map(|labels| { labels.get(&config().app_name_label).map(|s| s.clone()) })
            .unwrap_or(None)
    }

    fn new_from_container(container: &ContainerSummaryInternal) -> Result<Option<Self>> {
        if let Some(labels) = &container.labels {
            if !labels.contains_key(&config().app_name_label) {
                return Ok(None);
            }

            let app_name = labels[&config().app_name_label].clone();
            let port: u16 = labels[&config().port_label].parse()?;
            let external: bool = labels.get(&config().external_label).map(|b| b.parse()).unwrap_or(Ok(false))?;
            let auth_type = labels.get(&config().auth_label).map(|s| match s.as_str() {
                "oidc" => CaddyAuthType::Oidc,
                "headers" => CaddyAuthType::TrustedHeaders, 
                "none" => CaddyAuthType::None, 
                v @ _ => CaddyAuthType::Unknown(v.to_string())
            }).unwrap_or(CaddyAuthType::None);

            Ok(Some(AppData {
                app_name,
                containers: Vec::new(),
                port,
                external,
                auth_type,
            }))
        } else {
            return Ok(None)
        }
    }

    fn domain(&self) -> &str {
        if self.external { config().external_domain.as_str() } else { config().local_domain.as_str() }
    }

    fn auth(&self) -> &'static str {
        match self.auth_type { CaddyAuthType::TrustedHeaders => "import auth-headers", _ => "" }
    }

    fn format_local_caddy(&self) -> String {
        format!(indoc!("
            @{app_name} host {app_name}.{domain}
              handle @{app_name} {{
                handle /metrics {{
                  abort
                }}
                handle /metrics/* {{
                  abort
                }}
                reverse_proxy http://localhost:880
              }}
        "), app_name=self.app_name, domain=self.domain())
    }

    fn format_docker_caddy(&self) -> String {
        let targets = self.containers.iter().map(|adc| format!("http://{}:{}", adc.hostname, self.port)).collect::<Vec<String>>().join(" ");
        format!(indoc!("
            @{app_name} host {app_name}.{domain}
              handle @{app_name} {{
                handle /metrics {{
                  abort
                }}
                handle /metrics/* {{
                  abort
                }}
                {auth}
                reverse_proxy {targets}
              }}
        "), app_name=self.app_name, domain=self.domain(), auth=self.auth(), targets=targets)
    }
}

#[derive(Debug)]
struct AppContainerData {
    container_id: String,
    container_name: String,
    hostname: String,
}

impl AppContainerData {
    fn new_from_summary(summary: &ContainerSummaryInternal) -> Option<Self> {
        if let Some(labels) = &summary.labels {
            if !labels.contains_key(&config().app_name_label) {
                None
            } else {

                let hostname = summary.container_name.clone();
                let container_id = summary.id.clone();
                let container_name = summary.container_name.clone();

                Some(Self {
                    container_id,
                    container_name,
                    hostname,
                })
            }
        } else {
            None
        }
    }
}

struct Listener {
    app_data: ApplicationData,
}

impl Listener {
    fn new() -> Self {
        Self {
            app_data: HashMap::new(),
        }
    }

    async fn write_caddy_snippets(&self) -> Result<()> {
        let mut docker_hosts_file = File::options().create(true).write(true).truncate(true).open(config().docker_caddy.snippets_dir.join("docker-hosts"))?;
        let mut local_docker_hosts_file = File::options().create(true).write(true).truncate(true).open(config().local_caddy.snippets_dir.join("docker-hosts"))?;
        let mut external_hosts = Vec::new();
        let mut local_external_hosts = Vec::new();
        let mut internal_hosts = Vec::new();
        let mut local_internal_hosts = Vec::new();

        for (key, ad) in self.app_data.iter() {
            if ad.containers.is_empty() {
                warn!(app_name=key, "app is in the map but has no running containers...");
                continue;
            }

            if ad.external {
                //println!("writing line [{line}] to external");
                external_hosts.push(ad.format_docker_caddy());
                local_external_hosts.push(ad.format_local_caddy());
            } else {
                //println!("writing line [{line}] to internal");
                internal_hosts.push(ad.format_docker_caddy());
                local_internal_hosts.push(ad.format_local_caddy());
            };
        }
        write!(&mut docker_hosts_file, indoc!("
            (external_docker_hosts) {{
              {}
            }}

            (internal_docker_hosts) {{
              {}
            }}
            "), external_hosts.join("\n  "), internal_hosts.join("\n  "))?;

        write!(&mut local_docker_hosts_file, indoc!("
            (external_docker_hosts) {{
              {}
            }}

            (internal_docker_hosts) {{
              {}
            }}
            "), local_external_hosts.join("\n  "), local_internal_hosts.join("\n  "))?;

        docker_hosts_file.sync_all()?;
        local_docker_hosts_file.sync_all()?;

        self.reload_caddy().await?;

        Ok(())
    }

    async fn reload_local_caddy(&self, config: &CaddyConfig) -> Result<()> {
        info!("reloading local-caddy...");
        let exit_status = std::process::Command::new(&config.bin_path)
            .current_dir(config.config_dir.to_str().ok_or("unable to get local caddy config dir as string")?)
            .args(["reload"])
            .spawn()?
            .wait()?;

        if !exit_status.success() {
            error!(code=exit_status.code(), "unable to reload local Caddy");
            return Err(format!("unable to reload local Caddy - exited with status {}", exit_status.code().unwrap_or(-1)).into());
        }

        Ok(())
    }

    async fn reload_docker_caddy(&self, config: &CaddyConfig) -> Result<()> {
        info!("reloading docker-caddy...");
        let docker = new_docker()?;
        let opts = ContainerListOpts::builder().filter(vec![ContainerFilter::Name("caddy".to_string())]).build();
        let search_results = docker.containers().list(&opts).await?;
        if search_results.len() != 1 {
            return Err("expected only a single container with the caddy container name".into());
        }

        let caddy_container = docker.containers().get(search_results[0].id.as_ref().expect("containers must always have an ID"));

        let create_opts = ExecCreateOpts::builder()
            .working_dir(&config.config_dir)
            .attach_stdout(true)
            .attach_stderr(true)
            .command(vec!["sh", "-c", format!("DO_API_KEY=\"$(cat \"$DO_API_KEY_FILE\")\" {} reload", config.bin_path.to_str().ok_or("could not turn caddy docker bin path into string")?).as_str()])
            .build();
        let start_opts = ExecStartOpts::builder().build();

        let mut result = caddy_container.exec(&create_opts, &start_opts).await?;
        while let Some(chunk) = result.next().await {
            match chunk? {
                TtyChunk::StdIn(_) => unreachable!("never attached"),
                TtyChunk::StdOut(bytes) => info!("{}", str::from_utf8(&bytes).unwrap_or_default()),
                TtyChunk::StdErr(bytes) => warn!("{}", str::from_utf8(&bytes).unwrap_or_default()),
            }
        }

        Ok(())
    }

    async fn reload_caddy(&self) -> Result<()> {
        match config().docker_caddy.location {
            CaddyLocation::Local => self.reload_local_caddy(&config().docker_caddy).await?,
            CaddyLocation::Docker(_) => self.reload_docker_caddy(&config().docker_caddy).await?,
        }

        match config().local_caddy.location {
            CaddyLocation::Local => self.reload_local_caddy(&config().local_caddy).await?,
            CaddyLocation::Docker(_) => self.reload_docker_caddy(&config().local_caddy).await?,
        }

        Ok(())
    }

    async fn listen(&mut self) -> Result<()> {
        let docker = new_docker()?;

        let container_opts = ContainerListOpts::builder().build();
        info!("checking containers & building app data on startup");
        for container in docker.containers().list(&container_opts).await? {
            let container_id = container.id.as_ref().unwrap().to_string();
            let container = docker.containers().get(&container_id).inspect().await?;
            let container_summary = ContainerSummaryInternal::new_from_inspect(&container)?;

            info!(container_name=container_summary.container_name, "checking container...");
            if let Some(mut ad) = AppData::new_from_container(&container_summary)? {
                if let Some(acd) = AppContainerData::new_from_summary(&container_summary) {
                    info!(?ad, "adding app data");
                    ad.containers.push(acd);
                    self.app_data.insert(ad.app_name.clone(), ad);
                } else {
                    warn!(app_name=ad.app_name, "built AppData but not AppContainerData");
                }
            }
            else {
                debug!("container not exposed via Caddy annotations");
            }
        }

        //write_caddy_snippets(&app_data)?;
        self.write_caddy_snippets().await?;

        let opts = EventsOpts::builder().build();
        let mut events = docker.events(&opts);
        while let Some(event) = events.next().await {
            let event = event?;
            if let Some("container") = event.type_.as_ref().map(|s| s.as_str()) {
                if let Some(action) = event.action.as_ref().map(|s| s.as_str()) {
                    let event_summary = EventSummaryInternal::new_from_event(&event)?;
                    match action {
                        "create" => {
                            //info!(?event, "received container event");
                            info!(actor_id=event.actor.unwrap().id, "received container create event");
                            let container = docker.containers().get(&event_summary.id).inspect().await?;
                            let container_summary = ContainerSummaryInternal::new_from_inspect(&container)?;
                            if let Some(app_name) = AppData::name_from_summary(&container_summary) {
                                if let Some(ad) = self.app_data.get_mut(&app_name) { 
                                    if let Some(adc) = AppContainerData::new_from_summary(&container_summary) {
                                        ad.containers.push(adc);
                                    } else {
                                        warn!(app_name, "generated AppData but no AppContainerData!");
                                        continue;
                                    }
                                } else {
                                    if let Some(mut ad) = AppData::new_from_container(&container_summary)? {
                                        if let Some(adc) = AppContainerData::new_from_summary(&container_summary) {
                                            ad.containers.push(adc);
                                            self.app_data.insert(app_name.clone(), ad);
                                        } else {
                                            warn!(app_name, "generated AppData but no AppContainerData!");
                                            continue;
                                        }
                                    } else {
                                        warn!(app_name, "app found in map, but generated no AppData");
                                        continue;
                                    }
                                }
                                self.write_caddy_snippets().await?;
                            }
                        }
                        "destroy" => {
                            //info!(?event, "received container event");
                            info!(actor_id=event.actor.unwrap().id, "received container destroy event");
                            if let Some(app_name) = event_summary.app_name {
                                if let Some(ad) = self.app_data.get_mut(&app_name) {
                                    ad.containers.retain(|ad| ad.container_id != event_summary.id);
                                    self.write_caddy_snippets().await?;
                                } else {
                                    warn!(app_name, "no AppData found for event - app not registered?");
                                }
                            } else {
                                debug!("no app name found for event");
                            }
                        }
                        "rename" => {
                            //println!("received container rename event:\n{:?}", event);
                            info!(actor_id=event.actor.unwrap().id, "received container rename event");
                            if let Some(app_name) = event_summary.app_name {
                                if let Some(ad) = self.app_data.get_mut(&app_name) {
                                    ad.containers.iter_mut().filter(|ad| &ad.container_name == event_summary.old_name.as_ref().unwrap()).for_each(|ad| {
                                        ad.container_name = event_summary.container_name.clone();
                                        ad.hostname = event_summary.container_name.clone();
                                    });
                                    self.write_caddy_snippets().await?;
                                }
                            }
                        }
                        "update" => {
                            //println!("received container event:\n{:?}", event);
                            info!(actor_id=event.actor.unwrap().id, "received container update event");
                            //let container = docker.containers().get(&event_summary.id).inspect().await?;
                            //let container_summary = ContainerSummaryInternal::new_from_inspect(&container)?;
                            //let name = container_summary.container_name.clone();
                            //if let Some(ad) = app_data.get_mut(&name) {
                            //    if let Some(labels) = &container_summary.labels {
                            //        if !labels.contains_key(&config().app_name_label) {
                            //            ad.app_name = labels[&config().app_name_label].clone();
                            //            ad.hostname = name.clone();
                            //            ad.port = labels[&config().port_label].parse()?;
                            //            ad.external = labels[&config().external_label].parse()?;
                            //            ad.auth_type = labels.get(&config().auth_label).map(|s| match s.as_str() {
                            //                "oidc" => CaddyAuthType::Oidc,
                            //                "headers" => CaddyAuthType::TrustedHeaders, 
                            //                v @ _ => CaddyAuthType::Unknown(v.to_string())
                            //            }).unwrap_or(CaddyAuthType::None);

                            //            write_caddy_snippets(&app_data)?;
                            //        } else if let Some(_) = app_data.remove(&name) {
                            //            write_caddy_snippets(&app_data)?;
                            //        }
                            //    } else if let Some(_) = app_data.remove(&name) {
                            //        write_caddy_snippets(&app_data)?;
                            //    }
                            //} else if let Some(ad) = AppData::new_from_container(&container_summary)? {
                            //    app_data.insert(name, ad);
                            //    write_caddy_snippets(&app_data)?;
                            //}
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = config(); // init immediately to validate args, print help, etc.
    tracing_subscriber::fmt()
        .with_target(false)
        .pretty()
        .init();

    let mut listener = Listener::new();

    listener.listen().await?;
    
    Ok(())
}
