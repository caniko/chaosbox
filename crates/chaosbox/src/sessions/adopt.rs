//! Register a store as a campaign source.
//!
//! Adoption pins a database into the campaign as a named, digest-addressed
//! input so downstream steps reference `staging` rather than an ad-hoc path
//! that may have moved. It never writes to the adopted database: every read
//! goes through a read-only handle, and the only file it creates is the
//! adoption record itself.

use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Tables that exist only in the v1 schema. Their presence or absence is the
/// schema marker, because `user_version` reads 0 on every store here.
const V1_ONLY_TABLES: [&str; 5] = ["message", "part", "session", "session_share", "todo"];

/// Adopt a store under a stable name.
#[derive(Clone, Debug)]
pub struct AdoptOptions {
    /// Campaign root holding `adoption/`; defaults to
    /// `$CHAOSBOX_SESSION_CAMPAIGN`.
    pub root: Option<PathBuf>,
    /// Stable key the store is adopted under.
    pub name: String,
    /// Absolute path to the store.
    pub db: PathBuf,
    /// Admit a store that still has open file descriptors, recorded as
    /// `held: true` and excluded from count derivation.
    pub allow_held: bool,
}

/// An adoption record could not be written.
#[derive(Debug, thiserror::Error)]
pub enum AdoptError {
    /// No campaign root was given and the environment names none.
    #[error("no campaign root: pass --root or set CHAOSBOX_SESSION_CAMPAIGN")]
    MissingRoot,
    /// The database path is not absolute, so the record could not pin it.
    #[error("--db must be an absolute path, got {0}")]
    RelativePath(PathBuf),
    /// The store refused adoption: unhealthy, unrecognized, or held.
    #[error("refused to adopt {db}: {reason}")]
    Refused {
        /// Store that was refused.
        db: PathBuf,
        /// Which gate failed.
        reason: String,
    },
    /// The campaign or the database could not be read.
    #[error("{context}: {source}")]
    Io {
        /// What was being read or written.
        context: String,
        /// Underlying failure.
        source: std::io::Error,
    },
    /// The database could not be queried.
    #[error("cannot query {db}: {source}")]
    Sqlite {
        /// Store that could not be queried.
        db: PathBuf,
        /// Underlying failure.
        source: rusqlite::Error,
    },
}

/// The adoption record written to `<root>/adoption/<name>.json`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdoptionRecord {
    name: String,
    db_path: String,
    dev: String,
    bytes: u64,
    sha256: String,
    schema: SchemaRecord,
    counts: CountsRecord,
    health: HealthRecord,
    holders: HoldersRecord,
    held: bool,
    adopted_at: String,
    boundary_record: Option<String>,
}

/// Schema marker and table-set fingerprint, per the cutover tooling: the
/// marker discriminates v1 from v2, the fingerprint pins one store.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaRecord {
    marker: String,
    fingerprint: String,
    table_count: usize,
    user_version: Option<i64>,
}

/// Session and message counts keyed on the schema actually present; a table
/// the schema does not carry reports null rather than failing the record.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CountsRecord {
    sessions: Option<i64>,
    messages: Option<i64>,
}

/// Integrity results for the adopted store.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthRecord {
    quick_check: String,
    foreign_key_violations: usize,
}

/// Whether anything still holds the store open.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HoldersRecord {
    clear: bool,
    count: usize,
}

/// Adopt the store, returning the record that was written.
///
/// # Errors
///
/// Returns [`AdoptError::MissingRoot`] without a root, [`AdoptError::RelativePath`]
/// for a relative `--db`, [`AdoptError::Refused`] when the store fails a
/// gate, and [`AdoptError::Io`] or [`AdoptError::Sqlite`] when the campaign
/// or the store cannot be read.
pub fn adopt(options: &AdoptOptions) -> Result<serde_json::Value, AdoptError> {
    if !options.db.is_absolute() {
        return Err(AdoptError::RelativePath(options.db.clone()));
    }
    let root = match &options.root {
        Some(root) => root.clone(),
        None => std::env::var("CHAOSBOX_SESSION_CAMPAIGN")
            .map(PathBuf::from)
            .map_err(|_| AdoptError::MissingRoot)?,
    };

    let metadata = fs::metadata(&options.db).map_err(|source| AdoptError::Io {
        context: format!("cannot stat {}", options.db.display()),
        source,
    })?;
    let sha256 = file_sha256(&options.db).map_err(|source| AdoptError::Io {
        context: format!("cannot hash {}", options.db.display()),
        source,
    })?;

    let connection = Connection::open_with_flags(&options.db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|source| AdoptError::Sqlite {
            db: options.db.clone(),
            source,
        })?;
    let schema = schema_info(&connection).map_err(|source| AdoptError::Sqlite {
        db: options.db.clone(),
        source,
    })?;
    let counts = session_counts(&connection);
    let health = health(&connection).map_err(|source| AdoptError::Sqlite {
        db: options.db.clone(),
        source,
    })?;
    drop(connection);

    if health.quick_check != "ok" || health.foreign_key_violations != 0 {
        return Err(refuse(&options.db, "store is unhealthy"));
    }
    if schema.marker != "v1" && schema.marker != "v2" {
        return Err(refuse(&options.db, "schema marker is unrecognized"));
    }
    let holders = holders_of(&options.db);
    if !holders.is_empty() && !options.allow_held {
        return Err(refuse(&options.db, "writers still hold the store"));
    }

    let record = AdoptionRecord {
        name: options.name.clone(),
        db_path: options.db.display().to_string(),
        dev: format!("{:#x}", metadata.dev()),
        bytes: metadata.len(),
        sha256,
        schema,
        counts,
        health: HealthRecord {
            quick_check: health.quick_check,
            foreign_key_violations: health.foreign_key_violations,
        },
        holders: HoldersRecord {
            clear: holders.is_empty(),
            count: holders.len(),
        },
        held: options.allow_held,
        adopted_at: rfc3339_now(),
        boundary_record: None,
    };
    let value = serde_json::to_value(&record).map_err(|source| AdoptError::Io {
        context: "cannot serialize adoption record".to_string(),
        source: std::io::Error::other(source),
    })?;
    publish_record(&root, &options.name, &value)?;
    Ok(value)
}

/// Publish an adoption record atomically: same-directory temporary file at
/// `0600`, then a rename.
fn publish_record(root: &Path, name: &str, value: &serde_json::Value) -> Result<(), AdoptError> {
    let directory = root.join("adoption");
    fs::create_dir_all(&directory).map_err(|source| AdoptError::Io {
        context: format!("cannot create {}", directory.display()),
        source,
    })?;
    // The directory must not be group- or world-accessible: adoption records
    // name live store paths.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(|source| {
            AdoptError::Io {
                context: format!("cannot secure {}", directory.display()),
                source,
            }
        })?;
    }
    let path = directory.join(format!("{name}.json"));
    let temporary = directory.join(format!("{name}.{}.tmp", std::process::id()));
    let text = serde_json::to_string_pretty(value).map_err(|source| AdoptError::Io {
        context: "cannot serialize adoption record".to_string(),
        source: std::io::Error::other(source),
    })?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(text.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()
        })
        .map_err(|source| AdoptError::Io {
            context: format!("cannot write {}", temporary.display()),
            source,
        })?;
    fs::rename(&temporary, &path).map_err(|source| AdoptError::Io {
        context: format!("cannot publish {}", path.display()),
        source,
    })
}

/// Refusal with the store named.
fn refuse(db: &Path, reason: &str) -> AdoptError {
    AdoptError::Refused {
        db: db.to_path_buf(),
        reason: reason.to_string(),
    }
}

/// Streaming SHA-256 over a file.
fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(hex(byte >> 4));
        out.push(hex(byte & 0x0f));
    }
    Ok(out)
}

/// One lowercase hex digit for a nibble.
fn hex(nibble: u8) -> char {
    (if nibble < 10 {
        b'0' + nibble
    } else {
        b'a' + nibble - 10
    }) as char
}

/// Schema marker, table-set fingerprint, and user version from a read-only
/// handle. Mirrors the cutover tooling's `schemaInfo`, which the install
/// preconditions compare against.
fn schema_info(connection: &Connection) -> Result<SchemaRecord, rusqlite::Error> {
    let mut statement =
        connection.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
    let tables: Vec<String> = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let present: std::collections::BTreeSet<&str> = tables.iter().map(String::as_str).collect();
    let v1_count = V1_ONLY_TABLES
        .iter()
        .filter(|table| present.contains(**table))
        .count();
    let marker = if v1_count == V1_ONLY_TABLES.len() {
        "v1"
    } else if v1_count == 0 {
        "v2"
    } else {
        "unrecognized"
    };
    let mut hasher = Sha256::new();
    hasher.update(tables.join("\n").as_bytes());
    let fingerprint = hex_digest(&hasher.finalize());
    let user_version: Option<i64> = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .ok();
    Ok(SchemaRecord {
        marker: marker.to_string(),
        fingerprint,
        table_count: tables.len(),
        user_version,
    })
}

/// Session and message counts keyed on the schema actually present. Reading
/// a missing table yields `None` rather than an error, so counts never fail
/// a record the schema already describes.
fn session_counts(connection: &Connection) -> CountsRecord {
    let one = |sql: &str| {
        connection
            .prepare(sql)
            .and_then(|mut statement| statement.query_row([], |row| row.get::<_, i64>(0)))
            .ok()
    };
    CountsRecord {
        sessions: one("SELECT count(*) FROM session_v2")
            .or_else(|| one("SELECT count(*) FROM session")),
        messages: one("SELECT count(*) FROM session_message")
            .or_else(|| one("SELECT count(*) FROM message")),
    }
}

/// Integrity results: `quick_check` comes back `"ok"` on a sound database.
fn health(connection: &Connection) -> Result<HealthRecord, rusqlite::Error> {
    let quick_check: String = connection
        .prepare("PRAGMA quick_check")
        .and_then(|mut statement| statement.query_row([], |row| row.get(0)))?;
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    let mut foreign_key_violations = 0_usize;
    while rows.next()?.is_some() {
        foreign_key_violations += 1;
    }
    Ok(HealthRecord {
        quick_check,
        foreign_key_violations,
    })
}

/// Every `(pid, fd)` still holding the database or one of its sidecars.
/// The gate is a `/proc` scan, not an assumption that a stopped unit wrote
/// nothing; an unreadable `/proc` reads as no holders rather than failing
/// the adoption.
fn holders_of(database: &Path) -> Vec<(u32, u64)> {
    let target = database.to_string_lossy().into_owned();
    let sidecar = format!("{target}-");
    let mut found = Vec::new();
    let Ok(processes) = fs::read_dir("/proc") else {
        return found;
    };
    for process in processes.flatten() {
        let pid: u32 = match process.file_name().to_string_lossy().parse() {
            Ok(pid) => pid,
            Err(_) => continue,
        };
        let Ok(descriptors) = fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for descriptor in descriptors.flatten() {
            let link = fs::read_link(descriptor.path())
                .map(|link| link.to_string_lossy().into_owned())
                .unwrap_or_default();
            let live = link.strip_suffix(" (deleted)").unwrap_or(&link);
            if live == target || live.starts_with(&sidecar) {
                let fd: u64 = descriptor
                    .file_name()
                    .to_string_lossy()
                    .parse()
                    .unwrap_or(u64::MAX);
                found.push((pid, fd));
            }
        }
    }
    found
}

/// Lowercase hex over a digest.
fn hex_digest(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(hex(byte >> 4));
        out.push(hex(byte & 0x0f));
    }
    out
}

/// Current UTC time as RFC3339 seconds, without a date-time crate.
fn rfc3339_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let (year, month, day, hour, minute, second) = civil_from_seconds(seconds);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Gregorian date and time from Unix seconds (Howard Hinnant's algorithm).
fn civil_from_seconds(seconds: u64) -> (i64, i64, i64, i64, i64, i64) {
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let time = i64::try_from(seconds % 86_400).unwrap_or(0);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (
        if month <= 2 { year + 1 } else { year },
        month,
        day,
        time / 3600,
        (time % 3600) / 60,
        time % 60,
    )
}
