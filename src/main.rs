use std::{
    io::{self, Write},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use starlink::{
    client::{Client, Query},
    history, render,
};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Read-only local Starlink telemetry and interruptions",
    long_about = "Read-only local Starlink telemetry and interruptions. No account login, settings changes, reboot, stow, or speed tests. Firmware-dependent unofficial API."
)]
struct Args {
    /// Dish gRPC-Web endpoint (native gRPC port 9200 is not supported)
    #[arg(long, global = true, default_value = "http://192.168.100.1:9201")]
    endpoint: String,
    /// Request timeout in seconds
    #[arg(long, global = true, default_value = "5", value_parser = positive_seconds)]
    timeout: f64,
    /// Machine-readable JSON (watch emits one JSON object per line)
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Current dish telemetry and known active alerts
    Status,
    /// Retained history summary (sample-derived; not a speed test)
    Stats {
        /// Analyze only the newest N one-second samples
        #[arg(long, value_parser = positive_count)]
        window: Option<usize>,
    },
    /// Dish-reported outage events plus sample-derived complete-loss runs
    Interruptions {
        /// Filter completed explicit events shorter than this many seconds
        #[arg(long, default_value = "0", value_parser = nonnegative_seconds)]
        min_duration: f64,
        /// Window for sample analysis only; does not filter explicit events
        #[arg(long, value_parser = positive_count)]
        window: Option<usize>,
    },
    /// Decoded history JSON in device ring-buffer order; redirect to a private file
    History,
    /// Inspect obstruction grid (use --json for full SNR array)
    Obstructions,
    /// Hardware and firmware information (contains device identifiers)
    Info,
    /// Router telemetry; requires --router-endpoint explicitly
    Router {
        #[arg(long)]
        router_endpoint: String,
    },
    /// Wi-Fi client inventory; requires --router-endpoint explicitly
    Clients {
        #[arg(long)]
        router_endpoint: String,
    },
    /// Repeated status + history summaries; Ctrl-C stops, errors never mean zero loss
    Watch {
        /// Delay after each poll completes, seconds (minimum 1)
        #[arg(long, default_value = "2", value_parser = interval_seconds)]
        interval: f64,
        /// Stop after N attempts; omitted means until Ctrl-C
        #[arg(long, value_parser = positive_count)]
        count: Option<usize>,
        /// Analyze only the newest N samples per snapshot (snapshots overlap)
        #[arg(long, default_value = "60", value_parser = positive_count)]
        window: usize,
    },
    /// Full-screen live charts; q, Esc or Ctrl-C exits (requires a terminal)
    Dashboard {
        /// Delay after each completed poll, seconds
        #[arg(long, default_value = "1", value_parser = interval_seconds)]
        interval: f64,
        /// Requested newest one-second samples (1..=86400; limited by dish retention)
        #[arg(long, default_value = "300", value_parser = dashboard_window)]
        window: usize,
    },
    /// Analyze a saved inner history JSON file offline; never contacts the dish
    Analyze {
        file: PathBuf,
        #[arg(long, value_parser = positive_count)]
        window: Option<usize>,
    },
}

fn nonnegative_seconds(s: &str) -> Result<f64, String> {
    let n: f64 = s
        .parse()
        .map_err(|_| "expected seconds as a number".to_owned())?;
    if n.is_finite() && (0.0..=86400.0).contains(&n) {
        Ok(n)
    } else {
        Err("seconds must be finite and between 0 and 86400".to_owned())
    }
}
fn positive_seconds(s: &str) -> Result<f64, String> {
    let n = nonnegative_seconds(s)?;
    if n >= 0.001 {
        Ok(n)
    } else {
        Err("seconds must be at least 0.001".to_owned())
    }
}
fn interval_seconds(s: &str) -> Result<f64, String> {
    let n = positive_seconds(s)?;
    if n >= 1.0 {
        Ok(n)
    } else {
        Err("poll interval must be at least 1 second".to_owned())
    }
}
fn positive_count(s: &str) -> Result<usize, String> {
    let n = s
        .parse::<usize>()
        .map_err(|_| "expected a positive integer".to_owned())?;
    if n > 0 {
        Ok(n)
    } else {
        Err("count must be greater than zero".to_owned())
    }
}
fn dashboard_window(s: &str) -> Result<usize, String> {
    let n = positive_count(s)?;
    if n <= 86400 {
        Ok(n)
    } else {
        Err("dashboard window must be at most 86400 samples".into())
    }
}
fn emit(text: &str) -> Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{text}")?;
    out.flush()?;
    Ok(())
}
fn output(value: &Value, human: String, as_json: bool) -> Result<()> {
    emit(&if as_json {
        serde_json::to_string_pretty(value)?
    } else {
        human
    })
}
fn summary_output(summary: &history::Summary, as_json: bool) -> Result<()> {
    output(
        &serde_json::to_value(summary)?,
        render::summary(summary),
        as_json,
    )
}

async fn run(args: Args) -> Result<()> {
    if let Command::Analyze { file, window } = &args.command {
        let file =
            std::fs::File::open(file).with_context(|| format!("opening {}", file.display()))?;
        // A decoded history should fit easily; bound offline input as well as network input.
        use std::io::Read;
        let mut bytes = Vec::new();
        file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 * 1024 {
            bail!("history file exceeds 16 MiB limit");
        }
        let value: Value = serde_json::from_slice(&bytes).context("parsing saved history JSON")?;
        return summary_output(&history::analyze(&value, *window)?, args.json);
    }
    let endpoint = match &args.command {
        Command::Router { router_endpoint } | Command::Clients { router_endpoint } => {
            router_endpoint
        }
        _ => &args.endpoint,
    };
    let client = Client::new(endpoint, Duration::from_secs_f64(args.timeout))?;
    match args.command {
        Command::Status => {
            let value = client.query(Query::Status).await?;
            output(&value, render::status(&value), args.json)
        }
        Command::Stats { window } => summary_output(
            &history::analyze(&client.query(Query::History).await?, window)?,
            args.json,
        ),
        Command::Interruptions {
            min_duration,
            window,
        } => {
            let s = history::analyze(&client.query(Query::History).await?, window)?;
            let events: Vec<_> = s
                .outages
                .iter()
                .filter(|o| o.ongoing || o.duration_seconds >= min_duration)
                .collect();
            let v = json!({"sample_count":s.sample_count,"available_count":s.available_count,
                "full_loss_sample_seconds":s.full_loss_sample_seconds,"consecutive_full_loss_runs":s.consecutive_full_loss_runs,
                "outages":events,"min_duration_seconds":min_duration,
                "note":"window applies to samples only; explicit event timestamps are raw device nanoseconds"});
            output(&v, render::interruptions(&s, min_duration), args.json)
        }
        Command::History => emit(&serde_json::to_string_pretty(
            &client.query(Query::History).await?,
        )?),
        Command::Obstructions => {
            let value = client.query(Query::Obstructions).await?;
            output(&value, render::obstructions(&value)?, args.json)
        }
        Command::Info => {
            let value = client.query(Query::DeviceInfo).await?;
            output(
                &value,
                render::fields("Device information", &value),
                args.json,
            )
        }
        Command::Router { .. } => {
            let value = client.query(Query::RouterStatus).await?;
            output(
                &value,
                render::fields("Router telemetry (partial schema)", &value),
                args.json,
            )
        }
        Command::Clients { .. } => {
            let value = client.query(Query::Clients).await?;
            output(&value, render::clients(&value), args.json)
        }
        Command::Watch {
            interval,
            count,
            window,
        } => watch(&client, interval, count, window, args.json).await,
        Command::Dashboard { interval, window } => {
            if args.json {
                bail!("dashboard does not support --json; use watch --json instead");
            }
            starlink::dashboard::run(&client, Duration::from_secs_f64(interval), window).await
        }
        Command::Analyze { .. } => unreachable!(),
    }
}

async fn watch(
    client: &Client,
    interval: f64,
    count: Option<usize>,
    window: usize,
    as_json: bool,
) -> Result<()> {
    let mut attempts = 0;
    let mut failures = 0;
    let interrupted = tokio::signal::ctrl_c();
    tokio::pin!(interrupted);
    loop {
        let poll = async {
            let status = client.query(Query::Status).await?;
            let summary = history::analyze(&client.query(Query::History).await?, Some(window))?;
            Ok::<_, anyhow::Error>((status, summary))
        };
        let result = tokio::select! {
            signal = &mut interrupted => { signal.context("listening for Ctrl-C")?; return Ok(()); },
            result = poll => result,
        };
        attempts += 1;
        let observed = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
        match result {
            Ok((status, summary)) => {
                if as_json {
                    emit(&serde_json::to_string(
                        &json!({"observed_at_unix_seconds":observed,"ok":true,"status":status,"summary":summary}),
                    )?)?;
                } else {
                    emit(&format!(
                        "=== Observation {observed:.3} (Unix seconds) ===\n{}\n{}",
                        render::status(&status),
                        render::summary(&summary)
                    ))?;
                }
            }
            Err(error) => {
                failures += 1;
                let error = format!("{error:#}");
                if as_json {
                    emit(&serde_json::to_string(
                        &json!({"observed_at_unix_seconds":observed,"ok":false,"error":error}),
                    )?)?;
                } else {
                    emit(&format!(
                        "=== Observation {observed:.3}: telemetry unavailable ===\n{}\nNo network-health conclusion can be drawn from this poll.",
                        render::safe(&error)
                    ))?;
                }
            }
        }
        if count.is_some_and(|n| attempts >= n) {
            if failures > 0 {
                bail!("{failures} of {attempts} telemetry polls failed");
            }
            return Ok(());
        }
        tokio::select! {
            signal = &mut interrupted => { signal.context("listening for Ctrl-C")?; return Ok(()); },
            _ = tokio::time::sleep(Duration::from_secs_f64(interval)) => {},
        }
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run(Args::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error)
            if error.chain().any(|e| {
                e.downcast_ref::<io::Error>()
                    .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
            }) =>
        {
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {}", render::safe(&format!("{error:#}")));
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_contract() {
        use clap::CommandFactory;
        Args::command().debug_assert();
        for command in [
            "status",
            "stats",
            "interruptions",
            "history",
            "obstructions",
            "info",
            "watch",
        ] {
            assert!(Args::try_parse_from(["starlink", command, "--json"]).is_ok());
        }
    }
    #[test]
    fn invalid_and_mutating_commands_are_rejected() {
        for command in ["reboot", "stow", "speed-test", "set", "router", "clients"] {
            assert!(Args::try_parse_from(["starlink", command]).is_err());
        }
        for args in [
            vec!["starlink", "--timeout", "NaN", "status"],
            vec!["starlink", "watch", "--interval", "0.5"],
            vec!["starlink", "watch", "--count", "0"],
            vec!["starlink", "stats", "--window", "0"],
            vec!["starlink", "interruptions", "--min-duration", "inf"],
        ] {
            assert!(Args::try_parse_from(args).is_err());
        }
    }
}
