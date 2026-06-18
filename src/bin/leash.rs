use std::{collections::BTreeMap, fmt, net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{bail, Result};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use leash_harness::{
    capability::default_capability_descriptors,
    config::{resolve_config, AcceleratorBackend, ConfigRequest, PartialHarnessConfig},
    daemon::{
        spawn_daemon, stop_process, tail_file, tail_jsonl_file, RunRecord, RunRegistry,
        StopOutcome, DEFAULT_RUN_NAME,
    },
    module::default_module_graph,
    replay::{scaled_delay, ReplayEvent, ReplayEventKind, ReplayRecording, REPLAY_FORMAT_VERSION},
    stack::{built_in_stacks, find_stack, Stack, StackTransport},
    transport::StreamTransportBackend,
    types::RunLogEntry,
    Harness, HarnessConfig, Profile, TelemetryStreamFrame,
};
use serde::Serialize;
use tracing::{
    field::{Field, Visit},
    Event, Subscriber,
};
use tracing_subscriber::{
    fmt::{format::Writer, FmtContext, FormatEvent, FormatFields},
    registry::LookupSpan,
    EnvFilter,
};

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
    Record(RecordArgs),
    Replay(ReplayArgs),
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

    #[arg(long, value_enum)]
    stream_transport: Option<StreamTransportBackend>,

    #[arg(long)]
    replay_source: Option<PathBuf>,

    #[arg(long)]
    replay_speed: Option<f64>,

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

    #[arg(long, action = ArgAction::SetTrue)]
    resource_sampling: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    no_resource_sampling: bool,
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
struct RecordArgs {
    #[arg(short, long)]
    output: PathBuf,

    #[arg(long, default_value_t = 5)]
    samples: usize,

    #[arg(long, default_value_t = 50)]
    interval_ms: u64,

    #[command(flatten)]
    runtime: RuntimeArgs,
}

#[derive(Debug, Args)]
struct ReplayArgs {
    input: PathBuf,

    #[arg(long, default_value_t = 1.0)]
    speed: f64,
}

#[derive(Debug, Args)]
struct RunArgs {
    stack: Option<String>,

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

    #[arg(long)]
    module: Option<String>,

    #[arg(long, action = ArgAction::SetTrue)]
    json: bool,
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
    stack: String,

    #[arg(long, value_enum, default_value_t = GraphFormat::Json)]
    format: GraphFormat,

    #[arg(long, env = "LEASH_ROLE", default_value = "robot")]
    role: String,

    #[arg(long, value_enum, default_value_t = StreamTransportBackend::LocalPubsub)]
    stream_transport: StreamTransportBackend,
}

#[derive(Debug, Args)]
struct ShowConfigArgs {
    stack: Option<String>,

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
            print_stack_list(args.format)?;
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
        Command::Record(args) => {
            let output = record_stream(args, cli.config.clone()).await?;
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        Command::Replay(args) => {
            replay_file(args).await?;
        }
        #[cfg(any(feature = "http", feature = "mcp"))]
        Command::Run(args) => {
            run_stack(args, cli.config.clone()).await?;
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
            let text = if args.json || args.module.is_some() {
                tail_jsonl_file(&record.log_path, args.lines, args.module.as_deref())?
            } else {
                tail_file(&record.log_path, args.lines)?
            };
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
            let config_stack = args
                .stack
                .as_deref()
                .map(resolve_config_stack)
                .transpose()?;
            let resolved = resolve_config(
                ConfigRequest::from_process(
                    cli.config.clone(),
                    config_stack.as_ref().map(|stack| stack.profile),
                    cli_overrides,
                )
                .with_stack_defaults(
                    config_stack
                        .as_ref()
                        .map(|stack| stack.defaults.clone())
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

fn print_stack_list(format: ListFormat) -> Result<()> {
    let stacks = built_in_stacks();
    match format {
        ListFormat::Json => println!("{}", serde_json::to_string_pretty(&stacks)?),
        ListFormat::Table => {
            println!(
                "{:<22} {:<13} {:<9} {:<8} {:<28} COMMAND",
                "NAME", "PROFILE", "TRANSPORT", "HARDWARE", "FEATURES"
            );
            for stack in stacks {
                let features = stack.required_features.join(",");
                println!(
                    "{:<22} {:<13} {:<9} {:<8} {:<28} {}",
                    stack.name,
                    stack.profile.as_str(),
                    stack.transport.kind.as_str(),
                    if stack.hardware_required { "yes" } else { "no" },
                    features,
                    stack.command
                );
            }
        }
    }
    Ok(())
}

#[cfg(any(feature = "http", feature = "mcp"))]
async fn run_stack(args: RunArgs, config_path: Option<PathBuf>) -> Result<()> {
    let selection = RunSelection::from_args(&args)?;
    let config = config_from_args(
        args.runtime,
        args.listen,
        config_path,
        selection.stack.as_ref(),
    )?;

    match selection.transport {
        StackTransport::Http => {
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
                bail!("HTTP stacks require the 'http' feature");
            }
        }
        StackTransport::Mcp => {
            if args.daemon {
                bail!("daemon mode is only supported for HTTP stacks");
            }
            #[cfg(feature = "mcp")]
            {
                let harness = Harness::new(config)?;
                leash_harness::mcp::serve_stdio(harness).await?;
            }
            #[cfg(not(feature = "mcp"))]
            {
                let _ = config;
                bail!("MCP stacks require the 'mcp' feature");
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct RunSelection {
    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    name: String,
    stack: Option<Stack>,
    transport: StackTransport,
}

impl RunSelection {
    fn from_args(args: &RunArgs) -> Result<Self> {
        if let Some(name) = args.stack.as_deref() {
            if let Some(stack) = find_stack(name) {
                stack.validate()?;
                return Ok(Self {
                    name: args.name.clone().unwrap_or_else(|| stack.name.clone()),
                    transport: stack.transport.kind,
                    stack: Some(stack),
                });
            }

            return Ok(Self {
                name: args.name.clone().unwrap_or_else(|| name.to_string()),
                stack: None,
                transport: StackTransport::Http,
            });
        }

        Ok(Self {
            name: args
                .name
                .clone()
                .unwrap_or_else(|| DEFAULT_RUN_NAME.to_string()),
            stack: None,
            transport: StackTransport::Http,
        })
    }
}

#[derive(Debug)]
struct ConfigStack {
    profile: Profile,
    defaults: PartialHarnessConfig,
}

fn resolve_config_stack(name: &str) -> Result<ConfigStack> {
    if let Some(stack) = find_stack(name) {
        stack.validate()?;
        return Ok(ConfigStack {
            profile: stack.profile,
            defaults: stack.config_overrides,
        });
    }

    let profile = match name {
        "sim" => Profile::Sim,
        "replay" => Profile::Replay,
        "waveshare-ugv" => Profile::WaveshareUgv,
        other => {
            let stacks = built_in_stacks()
                .into_iter()
                .map(|stack| stack.name)
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "unknown stack or profile '{other}'; expected sim, replay, waveshare-ugv, or one of: {stacks}"
            );
        }
    };
    Ok(ConfigStack {
        profile,
        defaults: PartialHarnessConfig::default(),
    })
}

impl RuntimeArgs {
    fn into_partial_config(self) -> Result<PartialHarnessConfig> {
        if self.allow_untokened_drive && self.no_untokened_drive {
            bail!("use either --allow-untokened-drive or --no-untokened-drive, not both");
        }
        if self.resource_sampling && self.no_resource_sampling {
            bail!("use either --resource-sampling or --no-resource-sampling, not both");
        }
        let profile = if self.replay_source.is_some() && self.profile.is_none() {
            Some(Profile::Replay)
        } else {
            self.profile
        };
        Ok(PartialHarnessConfig {
            role: self.role,
            profile,
            stream_transport: self.stream_transport,
            replay_source: self.replay_source,
            replay_speed: self.replay_speed,
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
            resource_sampling: if self.resource_sampling {
                Some(true)
            } else if self.no_resource_sampling {
                Some(false)
            } else {
                None
            },
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

#[derive(Debug, Serialize)]
struct RecordOutput {
    ok: bool,
    format: &'static str,
    path: String,
    samples: usize,
    events: usize,
    profile: String,
}

async fn record_stream(args: RecordArgs, config_path: Option<PathBuf>) -> Result<RecordOutput> {
    if args.samples == 0 {
        bail!("record --samples must be at least 1");
    }
    let interval = Duration::from_millis(args.interval_ms);
    let config = config_from_args(args.runtime, None, config_path, None)?;
    let profile = config.profile.as_str().to_string();
    let harness = Harness::new(config)?;
    let mut events = Vec::with_capacity(args.samples * 4);

    for sample in 0..args.samples {
        let mut frame = harness.telemetry_stream_frame();
        let ts_ms = sample as u128 * args.interval_ms as u128;
        normalize_replay_frame_timestamps(&mut frame, ts_ms);
        let seq = sample as u64 * 4;

        events.push(ReplayEvent::new(
            ts_ms,
            seq,
            ReplayEventKind::Telemetry,
            serde_json::to_value(&frame)?,
        ));
        events.push(ReplayEvent::new(
            ts_ms,
            seq + 1,
            ReplayEventKind::Sensors,
            serde_json::to_value(&frame.telemetry.sensors)?,
        ));
        events.push(ReplayEvent::new(
            ts_ms,
            seq + 2,
            ReplayEventKind::Camera,
            serde_json::to_value(&frame.telemetry.sensors.camera)?,
        ));
        events.push(ReplayEvent::new(
            ts_ms,
            seq + 3,
            ReplayEventKind::Command,
            serde_json::to_value(&frame.command)?,
        ));

        if sample + 1 < args.samples {
            tokio::time::sleep(interval).await;
        }
    }

    let recording = ReplayRecording::new(events);
    recording.write_path(&args.output)?;
    Ok(RecordOutput {
        ok: true,
        format: REPLAY_FORMAT_VERSION,
        path: args.output.display().to_string(),
        samples: args.samples,
        events: recording.events().len(),
        profile,
    })
}

async fn replay_file(args: ReplayArgs) -> Result<()> {
    let recording = ReplayRecording::read_path(&args.input)?;
    let mut previous_ts_ms = None;
    for event in recording.events() {
        if let Some(previous_ts_ms) = previous_ts_ms {
            tokio::time::sleep(scaled_delay(previous_ts_ms, event.ts_ms, args.speed)?).await;
        }
        println!("{}", serde_json::to_string(event)?);
        previous_ts_ms = Some(event.ts_ms);
    }
    Ok(())
}

fn normalize_replay_frame_timestamps(frame: &mut TelemetryStreamFrame, ts_ms: u128) {
    frame.ts_ms = ts_ms;
    frame.telemetry.ts_ms = ts_ms;
    frame.telemetry.profile = "replay".to_string();
    frame.telemetry.source = "replay".to_string();
    frame.telemetry.sensors.raw_frame.source = "replay".to_string();
    frame.telemetry.sensors.raw_frame.last_ms = Some(ts_ms);
    frame.health.mode = "replay".to_string();
    frame.health.replay = true;
    frame.health.profile = "replay".to_string();
    frame.health.uptime_ms = ts_ms;
    frame.health.physical_actuation_enabled = false;
    frame.safety.physical_actuation_enabled = false;
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
    let pid = spawn_daemon(
        &std::env::current_exe()?,
        &record.args,
        &record.log_path,
        &record.name,
    )?;
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
    let pid = spawn_daemon(&std::env::current_exe()?, &args, &log_path, name)?;
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
        "--stream-transport".to_string(),
        config.stream_transport.as_str().to_string(),
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
    if let Some(path) = &config.replay_source {
        args.push("--replay-source".to_string());
        args.push(path.display().to_string());
        args.push("--replay-speed".to_string());
        args.push(config.replay_speed.to_string());
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
    if config.resource_sampling {
        args.push("--resource-sampling".to_string());
    } else {
        args.push("--no-resource-sampling".to_string());
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
    let config_stack = resolve_config_stack(&args.stack)?;
    let capabilities = default_capability_descriptors()
        .into_iter()
        .map(|descriptor| descriptor.name)
        .collect();
    let config = HarnessConfig {
        role: args.role.clone(),
        profile: config_stack.profile,
        stream_transport: args.stream_transport,
        ..HarnessConfig::default()
    };
    Ok(default_module_graph(&config, capabilities))
}

fn config_from_args(
    args: RuntimeArgs,
    listen: Option<SocketAddr>,
    config_path: Option<PathBuf>,
    stack: Option<&Stack>,
) -> Result<HarnessConfig> {
    let mut cli = args.into_partial_config()?;
    if let Some(listen) = listen {
        cli.listen = Some(listen);
    }
    Ok(resolve_config(
        ConfigRequest::from_process(config_path, stack.map(|stack| stack.profile), cli)
            .with_stack_defaults(
                stack
                    .map(|stack| stack.config_overrides.clone())
                    .unwrap_or_default(),
            ),
    )?
    .config)
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if let Some(run_id) = std::env::var("LEASH_RUN_ID")
        .ok()
        .filter(|run_id| !run_id.trim().is_empty())
    {
        tracing_subscriber::fmt()
            .event_format(JsonlEventFormat { run_id })
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    }
}

struct JsonlEventFormat {
    run_id: String,
}

impl<S, N> FormatEvent<S, N> for JsonlEventFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut visitor = JsonFieldVisitor::default();
        event.record(&mut visitor);
        let metadata = event.metadata();
        let entry = RunLogEntry {
            timestamp: leash_harness::daemon::now_ms(),
            run_id: self.run_id.clone(),
            module: metadata.target().to_string(),
            event: visitor
                .message
                .unwrap_or_else(|| metadata.name().to_string()),
            level: metadata.level().as_str().to_ascii_lowercase(),
            fields: visitor.fields,
        };
        let line = serde_json::to_string(&entry).map_err(|_| fmt::Error)?;
        writeln!(writer, "{line}")
    }
}

#[derive(Default)]
struct JsonFieldVisitor {
    message: Option<String>,
    fields: BTreeMap<String, serde_json::Value>,
}

impl JsonFieldVisitor {
    fn insert(&mut self, field: &Field, value: serde_json::Value) {
        if field.name() == "message" {
            if let Some(message) = value.as_str() {
                self.message = Some(message.to_string());
            }
            return;
        }
        self.fields.insert(field.name().to_string(), value);
    }
}

impl Visit for JsonFieldVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, serde_json::json!(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, serde_json::json!(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, serde_json::json!(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, serde_json::json!(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, serde_json::json!(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.insert(field, serde_json::json!(format!("{value:?}")));
    }
}
