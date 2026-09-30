//! Explicit PostgreSQL operator commands; never exposed as MCP mutations.
use std::path::PathBuf;
use clap::Subcommand;
use chaosbox_store::GraphQueries;
use chaosbox::postgres::{Capture, collect, load_capture, publish, write_capture};
use super::{TypeDbStore, typedb_config_from_env};

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Capture catalog metadata with fixed read-only SQL and a private receipt.
    Capture {
        #[arg(long)]
        database: String,
        #[arg(long)]
        schema: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Publish one source-bound receipt to `TypeDB`; no inference is involved.
    Publish {
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        database: String,
        #[arg(long)]
        schema: String,
    },
    /// Collect current catalog, persist immutable evidence, and publish to `TypeDB`.
    Collect {
        #[arg(long)]
        database: String,
        #[arg(long)]
        schema: String,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        output_directory: PathBuf,
    },
}

pub(super) async fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Capture {
            database,
            schema,
            output,
        } => {
            let capture = collect(&database, &schema).map_err(|e| e.to_string())?;
            write_capture(&output, &capture).map_err(|e| e.to_string())?;
            println!("{}", report(&capture));
        }
        Command::Publish {
            artifact,
            repo,
            database,
            schema,
        } => {
            let capture = load_capture(&artifact).map_err(|e| e.to_string())?;
            if capture.catalog.database != database || capture.catalog.schema != schema {
                return Err("PostgreSQL receipt does not match requested source".into());
            }
            publish_typedb(&capture, &repo).await?;
        }
        Command::Collect {
            database,
            schema,
            repo,
            output_directory,
        } => {
            let capture = collect(&database, &schema).map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&output_directory).map_err(|e| e.to_string())?;
            let artifact = output_directory.join(format!("{}.json", capture.digest));
            if artifact.exists() {
                if load_capture(&artifact).map_err(|e| e.to_string())? != capture {
                    return Err("existing PostgreSQL receipt disagrees with capture".into());
                }
            } else {
                write_capture(&artifact, &capture).map_err(|e| e.to_string())?;
            }
            publish_typedb(&capture, &repo).await?;
        }
    }
    Ok(())
}

fn report(capture: &Capture) -> serde_json::Value {
    serde_json::json!({"version":1,"producer":"postgres-catalog-v1","snapshot":capture.snapshot(),"capture":capture.digest,
        "database":capture.catalog.database,"schema":capture.catalog.schema,"role":capture.catalog.role,"observed_at":capture.observed_at,
        "records":capture.catalog.records.len(),"omissions":capture.catalog.omissions,"exhaustive":false})
}

async fn publish_typedb(capture: &Capture, repo: &str) -> Result<(), String> {
    let config = typedb_config_from_env()?;
    let mut reader = chaosbox_typedb::reader::TypeDbReader::new(config.clone());
    Box::pin(reader.connect())
        .await
        .map_err(|e| e.to_string())?;
    let active = Box::pin(reader.active_build(repo))
        .await
        .map_err(|e| e.to_string())?;
    if let Some(current) = active
        .as_ref()
        .and_then(|build| build.coverage.as_ref())
        .and_then(|coverage| coverage.catalog.as_ref())
    {
        if current.database != capture.catalog.database
            || current.schema != capture.catalog.schema
            || current.observed_at > capture.observed_at
        {
            return Err(
                "PostgreSQL publication would change source identity or regress collection time"
                    .into(),
            );
        }
    }
    let (predecessor, generation) =
        chaosbox::chain_publication(active.map(|build| (build.build_id, build.generation)))
            .map_err(|error| error.to_string())?;
    let mut store = TypeDbStore::new(config);
    let next = generation
        .checked_add(1)
        .ok_or("PostgreSQL publication generation exhausted")?;
    let build = Box::pin(publish(&mut store, capture, repo, next, predecessor))
        .await
        .map_err(|e| e.to_string())?;
    let mut value = report(capture);
    value["build_id"] = serde_json::json!(build.id);
    value["generation"] = serde_json::json!(build.generation);
    println!("{value}");
    Ok(())
}
