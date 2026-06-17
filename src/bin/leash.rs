use std::{net::SocketAddr, path::PathBuf};

use anyhow::{bail, Result};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use leash_harness::{
    capability::default_capability_descriptors,
    config::{resolve_config, ConfigRequest, PartialHarnessConfig},
    module::default_module_graph,
    Harness, HarnessConfig, Profile,
};
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
    Serve(Serve),
    Graph(GraphArgs),
    ShowConfig(ShowConfigArgs),
    Health(HttpTarget),
    Stop(HttpTarget),
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
struct GraphArgs {
    #[arg(default_value = "sim")]
    blueprint: String,

    #[arg(long, value_enum, default_value_t = GraphFormat::Json)]
    format: GraphFormat,

    #[arg(long, env = "LEASH_ROLE", default_value = "robot")]
    role: String,
}

#[derive(Debug, Args)]
struct ShowConfigArgs {
    #[arg(value_enum)]
    blueprint: Option<Profile>,

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
        Command::Serve(serve) => match serve.transport {
            #[cfg(feature = "mcp")]
            Transport::Mcp(args) => {
                let harness = Harness::new(config_from_args(args, None, cli.config.clone())?)?;
                leash_harness::mcp::serve_stdio(harness).await?;
            }
            #[cfg(feature = "http")]
            Transport::Http(args) => {
                let config = config_from_args(args.runtime, args.listen, cli.config.clone())?;
                let listen = config.listen;
                let harness = Harness::new(config)?;
                leash_harness::http::serve_http(harness, listen).await?;
            }
        },
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
            let resolved = resolve_config(ConfigRequest::from_process(
                cli.config.clone(),
                args.blueprint,
                cli_overrides,
            ))?;
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
        Command::Stop(target) => {
            let client = reqwest::Client::new();
            let value: serde_json::Value = client
                .post(format!("{}/motors/stop", target.url.trim_end_matches('/')))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
    }

    Ok(())
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
            ..PartialHarnessConfig::default()
        })
    }
}

fn graph_from_args(args: &GraphArgs) -> Result<leash_harness::ModuleGraph> {
    let profile = match args.blueprint.as_str() {
        "sim" => Profile::Sim,
        "waveshare-ugv" => Profile::WaveshareUgv,
        other => bail!("unknown graph target '{other}'; expected sim or waveshare-ugv"),
    };
    let capabilities = default_capability_descriptors()
        .into_iter()
        .map(|descriptor| descriptor.name)
        .collect();
    let config = HarnessConfig {
        role: args.role.clone(),
        profile,
        ..HarnessConfig::default()
    };
    Ok(default_module_graph(&config, capabilities))
}

fn config_from_args(
    args: RuntimeArgs,
    listen: Option<SocketAddr>,
    config_path: Option<PathBuf>,
) -> Result<HarnessConfig> {
    let mut cli = args.into_partial_config()?;
    if let Some(listen) = listen {
        cli.listen = Some(listen);
    }
    Ok(resolve_config(ConfigRequest::from_process(config_path, None, cli))?.config)
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
