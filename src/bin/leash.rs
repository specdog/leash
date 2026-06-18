use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{bail, Result};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use leash_harness::{
    capability::default_capability_descriptors,
    config::{resolve_config, AcceleratorBackend, ConfigRequest, PartialHarnessConfig},
    daemon::{
        spawn_daemon, stop_process, tail_file, RunRecord, RunRegistry, StopOutcome,
        DEFAULT_RUN_NAME,
    },
    module::default_module_graph,
    pawprint::{built_in_pawprints, find_pawprint, Pawprint, PawprintTransport},
    Harness, HarnessConfig, Profile,
};
use serde::Serialize;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "leash",
    version,
    about = "Composable local-LLM and robot harness"
)]
struct Cli {
    #[arg(long, env = "LEASH_CONFIG", global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    List(ListArgs),
    Serve(Serve),
    #[cfg(any(feature = "http", feature = "mcp"))]
    Run(RunArgs),
    Status(StatusArgs),
    Log(LogArgs),
    Restart(RestartArgs),
    Graph(GraphArgs),
    ShowConfig(ShowConfigArgs),
    Health(HttpTarget),
    Stop(StopArgs),
}

#[derive(Debug, Args)]
struct ListArgs {
    #[arg(long, value_enum, default_value_t = ListFormat::Table)]
    format: ListFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ListFormat {
    Table,
    Json,
}

#[derive(Debug, Args)]
struct Serve {
    #[command(subcommand)]
    transport: Transport,
}

#[derive(Debug, Subcommand)]
enum Transport {
    #[cfg(feature = "mcp")]
    Mcp(RuntimeArgs),
    #[cfg(feature = "http")]
    Http(HttpServeArgs),
}

#[derive(Debug, Args)]
struct RuntimeArgs {
    #[arg(long)]
    role: Option<String>,

    #[arg(long, value_enum)]
    profile: Option<Profile>,

    #[arg(long, action = ArgAction::SetTrue)]
    allow_untokened_drive: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    no_untokened_drive: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    allow_physical_actuation: bool,

    #[arg(long)]
    deadman_ms: Option<u64>,

    #[arg(long)]
    soft_odometry_limit_m: Option<f64>,

    #[arg(long)]
    serial_port: Option<String>,

    #[arg(long)]
    serial_baud: Option<u32>,

    #[arg(long, action = ArgAction::SetTrue)]
    drive_invert: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    drive_swap: bool,

    #[arg(long, value_enum)]
    accelerator: Option<AcceleratorBackend>,

    #[arg(long, action = ArgAction::SetTrue)]
    require_accelerator: bool,
}

#[derive(Debug, Args)]
struct HttpServeArgs {
    #[command(flatten)]
    runtime: RuntimeArgs,

    #[arg(long)]
    listen: Option<SocketAddr>,
}

#[derive(Debug, Args)]
struct HttpTarget {
    #[arg(long, env = "LEASH_URL", default_value = "http://127.0.0.1:8000")]
    url: String,
}

#[derive(Debug, Args)]
struct RunArgs {
    target: Option<String>,

    #[arg(long)]
    name: Option<String>,

    #[arg(long, action = ArgAction::SetTrue)]
    daemon: bool,

    #[command(flatten)]
    runtime: RuntimeArgs,

    #[arg(long)]
    listen: Option<SocketAddr>,
}

#[derive(Debug, Args)]
struct StatusArgs {
    name: Option<String>,
}

#[derive(Debug, Args)]
struct LogArgs {
    #[arg(default_value = DEFAULT_RUN_NAME)]
    name: String,

    #[arg(long, default_value_t = 80)]
    lines: usize,
}

#[derive(Debug, Args)]
struct StopArgs {
    #[arg(default_value = DEFAULT_RUN_NAME)]
    name: String,

    #[arg(long)]
    url: Option<String>,

    #[arg(long, default_value_t = 2_000)]
    graceful_timeout_ms: u64,
}

#[derive(Debug, Args)]
struct RestartArgs {
    #[arg(default_value = DEFAULT_RUN_NAME)]
    name: String,

    #[arg(long, default_value_t = 2_000)]
    graceful_timeout_ms: u64,
}

#[derive(Debug, Args)]
struct GraphArgs {
    #[arg(default_value = "sim")]
    pawprint: String,

    #[arg(long, value_enum, default_value_t = GraphFormat::Json)]
    format: GraphFormat,

    #[arg(long, env = "LEASH_ROLE", default_value = "robot")]
    role: String,
}

#[derive(Debug, Args)]
struct ShowConfigArgs {
    pawprint: Option<String>,

    #[command(flatten)]
    runtime: RuntimeArgs,

    #[arg(long)]
    listen: Option<SocketAddr>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum GraphFormat {
    Json,
    Dot,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Command::List(args) => {
            print_pawprint_list(args.format)?;
        }
        Command::Serve(serve) => match serve.transport {
            #[cfg(feature = "mcp")]
            Transport::Mcp(args) => {
                let harness =
                    Harness::new(config_from_args(args, None, cli.config.clone(), None)?)?;
                leash_harness::mcp::serve_stdio(harness).await?;
            }
            #[cfg(feature = "http")]
            Transport::Http(args) => {
                let config = config_from_args(args.runtime, args.listen, cli.config.clone(), None)?;
                let listen = config.listen;
                let harness = Harness::new(config)?;
                leash_harness::http::serve_http(harness, listen).await?;
            }
        },
        #[cfg(any(feature = "http", feature = "mcp"))]
        Command::Run(args) => {
            run_target(args, cli.config.clone()).await?;
        }
        Command::Status(args) => {
            let output = daemon_status(args.name.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        Command::Log(args) => {
            let registry = RunRegistry::from_env()?;
            let Some(record) = registry.read(&args.name)? else {
                bail!("run '{}' was not found", args.name);
            };
            let text = tail_file(&record.log_path, args.lines)?;
            if !text.is_empty() {
                println!("{text}");
            }
        }
        Command::Restart(args) => {
            let record =
                restart_daemon_run(&args.name, Duration::from_millis(args.graceful_timeout_ms))?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        Command::Graph(args) => {
            let graph = graph_from_args(&args)?;
            match args.format {
                GraphFormat::Json => println!("{}", serde_json::to_string_pretty(&graph)?),
                GraphFormat::Dot => print!("{}", graph.to_dot()),
            }
        }
        Command::ShowConfig(args) => {
            let mut cli_overrides = args.runtime.into_partial_config()?;
            if let Some(listen) = args.listen {
                cli_overrides.listen = Some(listen);
            }
            let target = args
                .pawprint
                .as_deref()
                .map(resolve_config_target)
                .transpose()?;
            let resolved = resolve_config(
                ConfigRequest::from_process(
                    cli.config.clone(),
                    target.as_ref().map(|target| target.profile),
                    cli_overrides,
                )
                .with_pawprint_defaults(
                    target
                        .as_ref()
                        .map(|target| target.defaults.clone())
                        .unwrap_or_default(),
                ),
            )?;
            println!("{}", serde_json::to_string_pretty(&resolved)?);
        }
        Command::Health(target) => {
            let value: serde_json::Value =
                reqwest::get(format!("{}/health", target.url.trim_end_matches('/')))
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Command::Stop(args) => {
            if let Some(url) = args.url {
                let value = stop_http_target(&url).await?;
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                let output =
                    stop_daemon_run(&args.name, Duration::from_millis(args.graceful_timeout_ms))?;
                println!("{}", serde_json::to_string_pretty(&output)?);
            }
        }
    }

    Ok(())
}

fn print_pawprint_list(format: ListFormat) -> Result<()> {
    let pawprints = built_in_pawprints();
    match format {
        ListFormat::Json => println!("{}", serde_json::to_string_pretty(&pawprints)?),
        ListFormat::Table => {
            println!(
                "{:<22} {:<13} {:<9} {:<8} {:<28} COMMAND",
                "NAME", "PROFILE", "TRANSPORT", "HARDWARE", "FEATURES"
            );
            for pawprint in pawprints {
                let features = pawprint.required_features.join(",");
                println!(
                    "{:<22} {:<13} {:<9} {:<8} {:<28} {}",
                    pawprint.name,
                    pawprint.profile.as_str(),
                    pawprint.transport.kind.as_str(),
                    if pawprint.hardware_required {
                        "yes"
                    } else {
                        "no"
                    },
                    features,
                    pawprint.command
                );
            }
        }
    }
    Ok(())
}

#[cfg(any(feature = "http", feature = "mcp"))]
async fn run_target(args: RunArgs, config_path: Option<PathBuf>) -> Result<()> {
    let selection = RunSelection::from_args(&args)?;
    let config = config_from_args(
        args.runtime,
        args.listen,
        config_path,
        selection.pawprint.as_ref(),
    )?;

    match selection.transport {
        PawprintTransport::Http => {
            #[cfg(feature = "http")]
            {
                if args.daemon {
                    let record = start_daemon_run(&selection.name, &config)?;
                    println!("{}", serde_json::to_string_pretty(&record)?);
                } else {
                    let listen = config.listen;
                    let harness = Harness::new(config)?;
                    leash_harness::http::serve_http(harness, listen).await?;
                }
            }
            #[cfg(not(feature = "http"))]
            {
                let _ = config;
                bail!("HTTP run targets require the 'http' feature");
            }
        }
        PawprintTransport::Mcp => {
            if args.daemon {
                bail!("daemon mode is only supported for HTTP run targets");
            }
            #[cfg(feature = "mcp")]
            {
                let harness = Harness::new(config)?;
                leash_harness::mcp::serve_stdio(harness).await?;
            }
            #[cfg(not(feature = "mcp"))]
            {
                let _ = config;
                bail!("MCP run targets require the 'mcp' feature");
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct RunSelection {
    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    name: String,
    pawprint: Option<Pawprint>,
    transport: PawprintTransport,
}

impl RunSelection {
    fn from_args(args: &RunArgs) -> Result<Self> {
        if let Some(target) = args.target.as_deref() {
            if let Some(pawprint) = find_pawprint(target) {
                pawprint.validate()?;
                return Ok(Self {
                    name: args.name.clone().unwrap_or_else(|| pawprint.name.clone()),
                    transport: pawprint.transport.kind,
                    pawprint: Some(pawprint),
                });
            }

            return Ok(Self {
                name: args.name.clone().unwrap_or_else(|| target.to_string()),
                pawprint: None,
                transport: PawprintTransport::Http,
            });
        }

        Ok(Self {
            name: args
                .name
                .clone()
                .unwrap_or_else(|| DEFAULT_RUN_NAME.to_string()),
            pawprint: None,
            transport: PawprintTransport::Http,
        })
    }
}

#[derive(Debug)]
struct ConfigTarget {
    profile: Profile,
    defaults: PartialHarnessConfig,
}

fn resolve_config_target(target: &str) -> Result<ConfigTarget> {
    if let Some(pawprint) = find_pawprint(target) {
        pawprint.validate()?;
        return Ok(ConfigTarget {
            profile: pawprint.profile,
            defaults: pawprint.config_overrides,
        });
    }

    let profile = match target {
        "sim" => Profile::Sim,
        "waveshare-ugv" => Profile::WaveshareUgv,
        other => {
            let pawprints = built_in_pawprints()
                .into_iter()
                .map(|pawprint| pawprint.name)
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "unknown config target '{other}'; expected sim, waveshare-ugv, or one of: {pawprints}"
            );
        }
    };
    Ok(ConfigTarget {
        profile,
        defaults: PartialHarnessConfig::default(),
    })
}

impl RuntimeArgs {
    fn into_partial_config(self) -> Result<PartialHarnessConfig> {
        if self.allow_untokened_drive && self.no_untokened_drive {
            bail!("use either --allow-untokened-drive or --no-untokened-drive, not both");
        }
        Ok(PartialHarnessConfig {
            role: self.role,
            profile: self.profile,
            allow_untokened_drive: if self.allow_untokened_drive {
                Some(true)
            } else if self.no_untokened_drive {
                Some(false)
            } else {
                None
            },
            allow_physical_actuation: self.allow_physical_actuation.then_some(true),
            deadman_ms: self.deadman_ms,
            soft_odometry_limit_m: self.soft_odometry_limit_m,
            serial_port: self.serial_port,
            serial_baud: self.serial_baud,
            drive_invert: self.drive_invert.then_some(true),
            drive_swap: self.drive_swap.then_some(true),
            accelerator: self.accelerator,
            require_accelerator: self.require_accelerator.then_some(true),
            ..PartialHarnessConfig::default()
        })
    }
}

#[derive(Debug, Serialize)]
struct StatusOutput {
    ok: bool,
    state_dir: String,
    stale_removed: Vec<String>,
    runs: Vec<RunStatus>,
}

#[derive(Debug, Serialize)]
struct RunStatus {
    name: String,
    pid: u32,
    running: bool,
    transport: String,
    profile: String,
    listen: String,
    log_path: String,
    args: Vec<String>,
}

#[derive(Debug, Serialize)]
struct StopOutput {
    ok: bool,
    name: String,
    pid: u32,
    outcome: StopOutcome,
}

#[cfg(feature = "http")]
fn start_daemon_run(name: &str, config: &HarnessConfig) -> Result<RunRecord> {
    let registry = RunRegistry::from_env()?;
    registry.cleanup_stale()?;
    if let Some(existing) = registry.read(name)? {
        bail!(
            "run '{}' is already registered with pid {}",
            name,
            existing.pid
        );
    }
    let args = serve_http_args(config);
    spawn_run_record(name, config, args)
}

fn restart_daemon_run(name: &str, graceful_timeout: Duration) -> Result<RunRecord> {
    let registry = RunRegistry::from_env()?;
    let Some(record) = registry.read(name)? else {
        bail!("run '{name}' was not found");
    };
    if leash_harness::daemon::is_process_alive(record.pid) {
        stop_process(record.pid, graceful_timeout)?;
    }
    registry.remove(name)?;
    let pid = spawn_daemon(&std::env::current_exe()?, &record.args, &record.log_path)?;
    let now = leash_harness::daemon::now_ms();
    let restarted = RunRecord {
        pid,
        started_at_ms: now,
        updated_at_ms: now,
        ..record
    };
    registry.write(&restarted)?;
    Ok(restarted)
}

fn stop_daemon_run(name: &str, graceful_timeout: Duration) -> Result<StopOutput> {
    let registry = RunRegistry::from_env()?;
    let Some(record) = registry.read(name)? else {
        bail!("run '{name}' was not found");
    };
    let outcome = stop_process(record.pid, graceful_timeout)?;
    registry.remove(name)?;
    Ok(StopOutput {
        ok: true,
        name: record.name,
        pid: record.pid,
        outcome,
    })
}

fn daemon_status(name: Option<&str>) -> Result<StatusOutput> {
    let registry = RunRegistry::from_env()?;
    let stale_removed = registry
        .cleanup_stale()?
        .into_iter()
        .map(|record| record.name)
        .collect::<Vec<_>>();
    let records = if let Some(name) = name {
        registry.read(name)?.into_iter().collect()
    } else {
        registry.list()?
    };
    let runs = records
        .into_iter()
        .map(|record| RunStatus {
            running: leash_harness::daemon::is_process_alive(record.pid),
            name: record.name,
            pid: record.pid,
            transport: record.transport,
            profile: record.profile,
            listen: record.listen,
            log_path: record.log_path.display().to_string(),
            args: record.args,
        })
        .collect::<Vec<_>>();
    Ok(StatusOutput {
        ok: true,
        state_dir: registry.root().display().to_string(),
        stale_removed,
        runs,
    })
}

#[cfg(feature = "http")]
fn spawn_run_record(name: &str, config: &HarnessConfig, args: Vec<String>) -> Result<RunRecord> {
    let registry = RunRegistry::from_env()?;
    let log_path = registry.log_path(name)?;
    let pid = spawn_daemon(&std::env::current_exe()?, &args, &log_path)?;
    let now = leash_harness::daemon::now_ms();
    let record = RunRecord {
        name: name.to_string(),
        pid,
        transport: "http".to_string(),
        profile: config.profile.as_str().to_string(),
        listen: config.listen.to_string(),
        log_path,
        args,
        started_at_ms: now,
        updated_at_ms: now,
    };
    registry.write(&record)?;
    Ok(record)
}

#[cfg(feature = "http")]
fn serve_http_args(config: &HarnessConfig) -> Vec<String> {
    let mut args = vec![
        "serve".to_string(),
        "http".to_string(),
        "--role".to_string(),
        config.role.clone(),
        "--profile".to_string(),
        config.profile.as_str().to_string(),
        "--listen".to_string(),
        config.listen.to_string(),
        "--deadman-ms".to_string(),
        config.deadman_ms.to_string(),
        "--soft-odometry-limit-m".to_string(),
        config.soft_odometry_limit_m.to_string(),
        "--serial-port".to_string(),
        config.serial_port.clone(),
        "--serial-baud".to_string(),
        config.serial_baud.to_string(),
        "--accelerator".to_string(),
        config.accelerator.as_str().to_string(),
    ];
    if config.allow_untokened_drive {
        args.push("--allow-untokened-drive".to_string());
    } else {
        args.push("--no-untokened-drive".to_string());
    }
    if config.allow_physical_actuation {
        args.push("--allow-physical-actuation".to_string());
    }
    if config.drive_invert {
        args.push("--drive-invert".to_string());
    }
    if config.drive_swap {
        args.push("--drive-swap".to_string());
    }
    if config.require_accelerator {
        args.push("--require-accelerator".to_string());
    }
    args
}

async fn stop_http_target(url: &str) -> Result<serde_json::Value> {
    let client = reqwest::Client::new();
    Ok(client
        .post(format!("{}/motors/stop", url.trim_end_matches('/')))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

fn graph_from_args(args: &GraphArgs) -> Result<leash_harness::ModuleGraph> {
    let target = resolve_config_target(&args.pawprint)?;
    let capabilities = default_capability_descriptors()
        .into_iter()
        .map(|descriptor| descriptor.name)
        .collect();
    let config = HarnessConfig {
        role: args.role.clone(),
        profile: target.profile,
        ..HarnessConfig::default()
    };
    Ok(default_module_graph(&config, capabilities))
}

fn config_from_args(
    args: RuntimeArgs,
    listen: Option<SocketAddr>,
    config_path: Option<PathBuf>,
    pawprint: Option<&Pawprint>,
) -> Result<HarnessConfig> {
    let mut cli = args.into_partial_config()?;
    if let Some(listen) = listen {
        cli.listen = Some(listen);
    }
    Ok(resolve_config(
        ConfigRequest::from_process(config_path, pawprint.map(|pawprint| pawprint.profile), cli)
            .with_pawprint_defaults(
                pawprint
                    .map(|pawprint| pawprint.config_overrides.clone())
                    .unwrap_or_default(),
            ),
    )?
    .config)
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
