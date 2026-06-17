use std::net::SocketAddr;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use leash_harness::{Harness, HarnessConfig, Profile};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "leash",
    version,
    about = "Composable local-LLM and robot harness"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve(Serve),
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
    #[arg(long, env = "LEASH_ROLE", default_value = "robot")]
    role: String,

    #[arg(long, env = "LEASH_PROFILE", value_enum, default_value_t = Profile::Sim)]
    profile: Profile,

    #[arg(long, env = "LEASH_ALLOW_UNTOKENED_DRIVE", default_value_t = true)]
    allow_untokened_drive: bool,

    #[arg(long, env = "LEASH_ALLOW_PHYSICAL_ACTUATION", default_value_t = false)]
    allow_physical_actuation: bool,

    #[arg(long, env = "LEASH_DEADMAN_MS", default_value_t = 400)]
    deadman_ms: u64,

    #[arg(long, env = "LEASH_SOFT_ODOMETRY_LIMIT_M", default_value_t = 0.0)]
    soft_odometry_limit_m: f64,

    #[arg(long, env = "LEASH_SERIAL_PORT", default_value = "/dev/ttyTHS1")]
    serial_port: String,

    #[arg(long, env = "LEASH_SERIAL_BAUD", default_value_t = 115_200)]
    serial_baud: u32,

    #[arg(long, env = "LEASH_DRIVE_INVERT", default_value_t = false)]
    drive_invert: bool,

    #[arg(long, env = "LEASH_DRIVE_SWAP", default_value_t = false)]
    drive_swap: bool,
}

#[derive(Debug, Args)]
struct HttpServeArgs {
    #[command(flatten)]
    runtime: RuntimeArgs,

    #[arg(long, env = "LEASH_LISTEN", default_value = "127.0.0.1:8000")]
    listen: SocketAddr,
}

#[derive(Debug, Args)]
struct HttpTarget {
    #[arg(long, env = "LEASH_URL", default_value = "http://127.0.0.1:8000")]
    url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Command::Serve(serve) => match serve.transport {
            #[cfg(feature = "mcp")]
            Transport::Mcp(args) => {
                let harness = Harness::new(config_from_args(args, None)?)?;
                leash_harness::mcp::serve_stdio(harness).await?;
            }
            #[cfg(feature = "http")]
            Transport::Http(args) => {
                let listen = args.listen;
                let harness = Harness::new(config_from_args(args.runtime, Some(listen))?)?;
                leash_harness::http::serve_http(harness, listen).await?;
            }
        },
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

fn config_from_args(args: RuntimeArgs, listen: Option<SocketAddr>) -> Result<HarnessConfig> {
    Ok(HarnessConfig {
        role: args.role,
        profile: args.profile,
        listen: listen.unwrap_or_else(|| HarnessConfig::default().listen),
        allow_untokened_drive: args.allow_untokened_drive,
        allow_physical_actuation: args.allow_physical_actuation,
        deadman_ms: args.deadman_ms,
        soft_odometry_limit_m: args.soft_odometry_limit_m,
        serial_port: args.serial_port,
        serial_baud: args.serial_baud,
        drive_invert: args.drive_invert,
        drive_swap: args.drive_swap,
    })
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
