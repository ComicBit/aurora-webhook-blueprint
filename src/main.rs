mod config;
mod dispatch;
mod exports;
mod intake;
mod model;
mod security;
mod server;
mod store;

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use config::Config;
use model::{BackfillPlan, LegacyRecord};
use std::{
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
};
use store::{Mode, Store};

#[derive(Parser)]
#[command(
    version,
    about = "Private durable Aurora meeting inbox and independent destinations"
)]
struct Cli {
    #[arg(long, global = true, default_value = "config.json")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Generate a new signing key. Output is sensitive; never put it in Git or shared logs.
    Keygen,
    /// Review the proposed configuration without activating anything.
    Plan,
    /// Initialize/apply the reviewed configuration, with the receiver stopped.
    Apply {
        #[arg(long)]
        approve: bool,
    },
    /// Check local destination configuration, without calling a model or remote endpoint.
    Doctor,
    Serve,
    /// Redacted status; safe to inspect while the daemon is running.
    Status,
    List {
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Print a retained exact original event (sensitive content).
    Show {
        event_id: String,
    },
    /// Print a retained destination result (sensitive content).
    Result {
        delivery_id: String,
    },
    /// Print a retained result imported from the stopped Go receiver (sensitive content).
    LegacyResult {
        event_id: String,
    },
    Delete {
        event_id: String,
    },
    Retry {
        delivery_id: String,
        #[arg(long)]
        acknowledge_duplicate_risk: bool,
    },
    Expire,
    /// Export normalized JSONL, not signed-raw backup. Requires a NEW private output file.
    Export {
        #[arg(long)]
        output: PathBuf,
    },
    Backup {
        #[arg(long)]
        output: PathBuf,
    },
    /// Import the explicit legacy Go migration format as held history, never runnable work.
    ImportGo {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        approve: bool,
    },
    #[command(subcommand)]
    Backfill(Backfill),
}
#[derive(Subcommand)]
enum Backfill {
    /// Freeze eligible retained event IDs in an immutable, locally recorded approval plan.
    Preview {
        destination_id: String,
        #[arg(long)]
        start: String,
        #[arg(long)]
        end: String,
        #[arg(long)]
        all_revisions: bool,
        #[arg(long)]
        output: PathBuf,
    },
    Apply {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        approve: bool,
    },
}
fn print_json(value: &impl serde::Serialize) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    out.write_all(b"\n")?;
    Ok(())
}
fn absolute_output(path: &Path) -> Result<PathBuf> {
    security::absolute_clean(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}
fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    std::fs::File::open(path)?
        .take(max as u64 + 1)
        .read_to_end(&mut data)?;
    ensure!(data.len() <= max, "input exceeds limit");
    Ok(data)
}
fn timestamp(value: &str) -> Result<i64> {
    Ok(
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
            .context("time must be RFC3339 with explicit timezone")?
            .unix_timestamp(),
    )
}
#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn run(cli: Cli) -> Result<()> {
    if matches!(cli.command, Command::Keygen) {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let mut key = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut key)?;
        println!("whsec_{}", STANDARD.encode(key));
        return Ok(());
    }
    let cfg = Config::load(&cli.config)?;
    match cli.command {
        Command::Keygen => unreachable!(),
        Command::Plan => print_json(
            &serde_json::json!({"config_digest":cfg.digest()?,"configuration":cfg,"warning":"Review destination content disclosure, target mode and authority. Apply never sends historical content automatically. Folder copies and provider histories have separate retention."}),
        ),
        Command::Apply { approve } => {
            ensure!(
                approve,
                "apply requires --approve after reviewing plan; stop the receiver first"
            );
            let store = Store::open(&cfg, Mode::Apply)?;
            print_json(
                &serde_json::json!({"applied_config_digest":store.config.digest()?,"historical_deliveries_created":0}),
            )
        }
        Command::Doctor => {
            let checks:Vec<_>=cfg.destinations.iter().map(|d| {
                let check=dispatch::preflight(d);
                serde_json::json!({"destination_id":d.id,"enabled":d.enabled,"local_preflight":if check.is_ok(){"passed"}else{"failed"},"note":check.err().map(|e|e.to_string()),"live_provider_validation":"not_run"})
            }).collect();
            print_json(
                &serde_json::json!({"checks":checks,"network_or_inference_calls":0,"note":"Local inspection is not actual agent policy, authentication or app/VPN delivery proof."}),
            )
        }
        Command::Serve => server::serve(cfg, intake::Secrets::load()?).await,
        Command::Status => {
            let s = Store::open(&cfg, Mode::ReadOnly)?;
            print_json(&s.status(model::now())?)
        }
        Command::List { after, limit } => {
            ensure!((1..=100).contains(&limit), "list limit must be 1..100");
            let s = Store::open(&cfg, Mode::ReadOnly)?;
            print_json(&s.list(after.as_deref(), limit, model::now())?)
        }
        Command::Show { event_id } => {
            let s = Store::open(&cfg, Mode::ReadOnly)?;
            std::io::stdout()
                .lock()
                .write_all(&s.raw(&event_id, model::now())?)?;
            Ok(())
        }
        Command::Result { delivery_id } => {
            let s = Store::open(&cfg, Mode::ReadOnly)?;
            std::io::stdout()
                .lock()
                .write_all(&s.result(&delivery_id, model::now())?)?;
            Ok(())
        }
        Command::LegacyResult { event_id } => {
            let s = Store::open(&cfg, Mode::ReadOnly)?;
            std::io::stdout()
                .lock()
                .write_all(&s.legacy_result(&event_id, model::now())?)?;
            Ok(())
        }
        Command::Delete { event_id } => {
            let mut s = Store::open(&cfg, Mode::Admin)?;
            s.delete(&event_id, model::now())?;
            println!(
                "Local content deleted; finite dedupe metadata remains. Destination copies were not erased."
            );
            Ok(())
        }
        Command::Retry {
            delivery_id,
            acknowledge_duplicate_risk,
        } => {
            let mut s = Store::open(&cfg, Mode::Admin)?;
            s.retry(&delivery_id, acknowledge_duplicate_risk, model::now())?;
            println!(
                "Explicit repeat authorized; original expiry unchanged. External effects may be duplicated."
            );
            Ok(())
        }
        Command::Expire => {
            let mut s = Store::open(&cfg, Mode::Admin)?;
            s.expire(model::now())?;
            print_json(&s.status(model::now())?)
        }
        Command::Backup { output } => {
            let s = Store::open(&cfg, Mode::Admin)?;
            s.backup(&absolute_output(&output)?)?;
            println!(
                "Consistent private backup created. Restore only with destinations disabled and external effects reconciled."
            );
            Ok(())
        }
        Command::Export { output } => {
            let s = Store::open(&cfg, Mode::ReadOnly)?;
            let output = absolute_output(&output)?;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&output)
                .context("export output must be a new private file")?;
            let mut cursor = None;
            let mut count = 0;
            let mut unavailable = 0;
            loop {
                let records = s.list(cursor.as_deref(), 100, model::now())?;
                if records.is_empty() {
                    break;
                }
                for row in &records {
                    if let Ok(raw) = s.raw(&row.event_id, model::now()) {
                        let event: serde_json::Value =
                            serde_json::from_slice(&raw).context("stored event is invalid")?;
                        serde_json::to_writer(
                            &mut f,
                            &serde_json::json!({"export_schema_version":1,"receipt":row,"event":event}),
                        )?;
                        f.write_all(b"\n")?;
                        count += 1;
                    } else {
                        unavailable += 1;
                    }
                }
                cursor = records.last().map(|r| r.receipt_id.clone());
            }
            f.sync_all()?;
            std::fs::File::open(output.parent().context("invalid output parent")?)?.sync_all()?;
            print_json(
                &serde_json::json!({"exported":count,"unavailable":unavailable,"format":"normalized_jsonl_not_raw_backup"}),
            )
        }
        Command::ImportGo { input, approve } => {
            ensure!(
                approve,
                "import requires --approve; all imported content remains held history"
            );
            let mut s = Store::open(&cfg, Mode::Admin)?;
            let mut file = std::io::BufReader::new(std::fs::File::open(input)?);
            let mut count = 0;
            loop {
                let mut line = Vec::new();
                let read = std::io::Read::by_ref(&mut file)
                    .take((13 << 20) + 1)
                    .read_until(b'\n', &mut line)?;
                if read == 0 {
                    break;
                }
                ensure!(line.len() <= 13 << 20, "legacy record exceeds limit");
                let row: LegacyRecord = serde_json::from_slice(&line)
                    .map_err(|_| anyhow::anyhow!("invalid legacy migration record"))?;
                s.import_legacy(row, model::now())
                    .context("legacy import stopped; inspect report before retry")?;
                count += 1;
            }
            print_json(&serde_json::json!({"imported_history":count,"created_deliveries":0}))
        }
        Command::Backfill(Backfill::Preview {
            destination_id,
            start,
            end,
            all_revisions,
            output,
        }) => {
            let mut s = Store::open(&cfg, Mode::Admin)?;
            let plan = s.preview(
                &destination_id,
                timestamp(&start)?,
                timestamp(&end)?,
                all_revisions,
                model::now(),
            )?;
            let path = absolute_output(&output)?;
            let bytes = serde_json::to_vec_pretty(&plan)?;
            security::write_private_new(&path, &bytes)?;
            print_json(&plan)
        }
        Command::Backfill(Backfill::Apply { plan, approve }) => {
            ensure!(
                approve,
                "backfill apply requires --approve after reviewing exact selected events"
            );
            let plan: BackfillPlan = serde_json::from_slice(&read_bounded(&plan, 1 << 20)?)
                .map_err(|_| anyhow::anyhow!("invalid backfill plan"))?;
            let mut s = Store::open(&cfg, Mode::Admin)?;
            let count = s.approve(&plan, model::now())?;
            print_json(&serde_json::json!({"created_deliveries":count,"retention_extended":false}))
        }
    }
}
