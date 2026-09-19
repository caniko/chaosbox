//! Read-only `TypeDB` [`GelQueries`](chaosbox_gel::GelQueries) implementation.
//!
//! Every read is scoped to one pinned build id through the membership
//! relations: rows outside the build are invisible by construction. Read
//! transactions are used throughout; a mutation query in one fails
//! server-side (`TSV9`, proven). Ordering matches the reference surface:
//! entity lists sort by qualified name, relationship and evidence lists are
//! sets (sorted by id here for determinism).
//!
//! Search preserves the Gel `ilike` contract through the `name-fold`
//! columns: the caller-side `like` pattern is unescaped to a literal
//! needle, folded, and matched with `contains` (`TypeQL` `like` is
//! case-sensitive and has no case-insensitive form). An empty relation-type
//! filter matches nothing.

use std::collections::BTreeMap;

use chaosbox_gel::{BuildRow, EntityRow, EndpointRef, EvidenceRow, GelError, GelQueries, RelRow};
use typedb_driver::{Address, Addresses, Credentials, DriverOptions, DriverTlsConfig, TypeDBDriver};

use crate::common::{TypeDbConfig, col_bool, col_int, col_string, driver_error, read_rows};
use crate::encode::{int_lit, str_lit};

/// Entity attribute columns selected by every member query.
const ENTITY_COLS: &[&str] = &["id", "kind", "repo", "snap", "file", "name", "qn"];
/// Relationship columns: header plus endpoint ids.
const REL_COLS: &[&str] = &["r", "rt", "fid", "tid"];

/// Project an attribute column map into an [`EntityRow`].
fn row_to_entity(
    row: &BTreeMap<String, typedb_driver::concept::Value>,
) -> Result<EntityRow, GelError> {
    Ok(EntityRow {
        entity_id: col_string(row, "id")?,
        kind: col_string(row, "kind")?,
        repo: col_string(row, "repo")?,
        snapshot: col_string(row, "snap")?,
        file: col_string(row, "file")?,
        name: col_string(row, "name")?,
        qualified_name: col_string(row, "qn")?,
    })
}

/// Project an attribute column map into a [`RelRow`].
fn row_to_rel(row: &BTreeMap<String, typedb_driver::concept::Value>) -> Result<RelRow, GelError> {
    Ok(RelRow {
        rel_id: col_string(row, "r")?,
        rel_type: col_string(row, "rt")?,
        from_entity: EndpointRef {
            entity_id: col_string(row, "fid")?,
        },
        to_entity: EndpointRef {
            entity_id: col_string(row, "tid")?,
        },
    })
}

/// Undo `%`-wrapping and `\` escapes of a `like` pattern into a literal
/// substring needle (mirrors `unescape_like` in `chaosbox-gel`).
fn unescape_like(like: &str) -> String {
    let mut out = String::with_capacity(like.len());
    let mut chars = like.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else if c != '%' {
            out.push(c);
        }
    }
    out
}

/// Read-only `TypeDB` query handle: one pinned database, read transactions.
pub struct TypeDbReader {
    config: TypeDbConfig,
    driver: Option<TypeDBDriver>,
}

impl TypeDbReader {
    /// A disconnected reader; connects on [`TypeDbReader::connect`].
    #[must_use]
    pub fn new(config: TypeDbConfig) -> Self {
        Self {
            config,
            driver: None,
        }
    }

    /// Connect the driver. Unlike the store, the read path never creates
    /// the database: a missing database is a client error the caller maps
    /// to the pending contract.
    pub async fn connect(&mut self) -> Result<(), GelError> {
        if self.driver.is_none() {
            let address: Address = self
                .config
                .address
                .parse()
                .map_err(|e| GelError::Client(format!("bad address: {e}")))?;
            let driver = TypeDBDriver::new(
                Addresses::from_address(address),
                Credentials::new(&self.config.username, &self.config.password),
                DriverOptions::new(DriverTlsConfig::disabled()),
            )
            .await
            .map_err(driver_error)?;
            if !driver
                .databases()
                .contains(&self.config.database)
                .await
                .map_err(driver_error)?
            {
                return Err(GelError::Client(format!(
                    "database {} not found",
                    self.config.database
                )));
            }
            self.driver = Some(driver);
        }
        Ok(())
    }

    /// Borrow the connected driver or report a client error.
    fn driver(&self) -> Result<&TypeDBDriver, GelError> {
        self.driver
            .as_ref()
            .ok_or_else(|| GelError::Client("TypeDbReader disconnected".into()))
    }

    /// Schema presence probe: true when the Chaosbox schema is applied
    /// (the marker query executes, rows or not), false when the schema
    /// types are unknown (`INF2`: migrations have not applied). Connection
    /// failures propagate as client errors.
    pub async fn probe(&self) -> Result<bool, GelError> {
        use typedb_driver::{TransactionOptions, TransactionType, answer::QueryAnswer};
        use crate::common::READ_TIMEOUT;
        let driver = self.driver()?;
        let tx = driver
            .transaction_with_options(
                &self.config.database,
                TransactionType::Read,
                TransactionOptions::new().transaction_timeout(READ_TIMEOUT),
            )
            .await
            .map_err(driver_error)?;
        match tx
            .query("match $x isa active-pointer; select $x; limit 1;")
            .await
        {
            Ok(answer) => {
                // Drain with the write helper's shape; rows are irrelevant.
                match answer {
                    QueryAnswer::ConceptRowStream(_, stream) => {
                        use futures::TryStreamExt;
                        let rows: Vec<_> = stream.try_collect().await.map_err(driver_error)?;
                        let _ = rows.len();
                        Ok(true)
                    }
                    _ => Ok(true),
                }
            }
            Err(e) if e.code() == "INF2" => Ok(false),
            Err(e) => Err(driver_error(e)),
        }
    }

    /// Member entities of one build, sorted by qualified name.
    async fn members(
        &self,
        build_id: &str,
        limit: Option<i64>,
    ) -> Result<Vec<EntityRow>, GelError> {
        let mut q = format!(
            "match (build: $b, member: $e) isa node-membership; $b isa graph-build, has build-id {}; $e isa code-entity, has entity-id $id, has kind $kind, has repo-name $repo, has snapshot-id $snap, has file $file, has name $name, has qualified-name $qn; select $id, $kind, $repo, $snap, $file, $name, $qn; sort $qn;",
            str_lit(build_id)
        );
        if let Some(n) = limit {
            q.push_str(" limit ");
            q.push_str(&int_lit(n.max(0)));
            q.push(';');
        }
        let rows = read_rows(self.driver()?, &self.config.database, &q, ENTITY_COLS).await?;
        rows.iter().map(row_to_entity).collect()
    }

    /// Member relationships of one build with endpoint ids, sorted by rel id.
    async fn member_rels(
        &self,
        build_id: &str,
        limit: Option<i64>,
    ) -> Result<Vec<RelRow>, GelError> {
        let mut q = format!(
            "match (build: $b, edge: $rel) isa edge-membership; $b isa graph-build, has build-id {}; $rel isa relationship (from-entity: $f, to-entity: $t), has rel-id $r, has rel-type $rt; $f isa code-entity, has entity-id $fid; $t isa code-entity, has entity-id $tid; select $r, $rt, $fid, $tid; sort $r;",
            str_lit(build_id)
        );
        if let Some(n) = limit {
            q.push_str(" limit ");
            q.push_str(&int_lit(n.max(0)));
            q.push(';');
        }
        let rows = read_rows(self.driver()?, &self.config.database, &q, REL_COLS).await?;
        rows.iter().map(row_to_rel).collect()
    }
}

#[async_trait::async_trait]
impl GelQueries for TypeDbReader {
    async fn active_build(&self, repo: &str) -> Result<Option<BuildRow>, GelError> {
        let q = format!(
            "match $p isa active-pointer, has repo-name {}, has build-id $b; $g isa graph-build, has build-id $b, has generation $gen, has status $st; select $b, $gen, $st;",
            str_lit(repo)
        );
        let rows = read_rows(
            self.driver()?,
            &self.config.database,
            &q,
            &["b", "gen", "st"],
        )
        .await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(BuildRow {
            build_id: col_string(&row, "b")?,
            generation: col_int(&row, "gen")?,
            status: col_string(&row, "st")?,
        }))
    }

    async fn search_entities(
        &self,
        build_id: &str,
        like: &str,
        limit: i64,
    ) -> Result<Vec<EntityRow>, GelError> {
        let needle = unescape_like(like).to_lowercase();
        let lim = int_lit(limit.max(0));
        // Two bounded subqueries (name fold, qualified-name fold), each
        // sorted and capped; the Rust merge below yields the true top-N.
        let mut merged: BTreeMap<String, EntityRow> = BTreeMap::new();
        for col in ["name-fold", "qualified-name-fold"] {
            let q = format!(
                "match (build: $b, member: $e) isa node-membership; $b isa graph-build, has build-id {}; $e isa code-entity, has {col} $hit, has entity-id $id, has kind $kind, has repo-name $repo, has snapshot-id $snap, has file $file, has name $name, has qualified-name $qn; $hit contains {}; select $id, $kind, $repo, $snap, $file, $name, $qn; sort $qn; limit {lim};",
                str_lit(build_id),
                str_lit(&needle)
            );
            let rows = read_rows(self.driver()?, &self.config.database, &q, ENTITY_COLS).await?;
            for row in &rows {
                let e = row_to_entity(row)?;
                merged.insert(e.entity_id.clone(), e);
            }
        }
        let n = usize::try_from(limit.max(0)).unwrap_or(0);
        let mut v: Vec<EntityRow> = merged.into_values().collect();
        v.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        v.truncate(n);
        Ok(v)
    }

    async fn entity_by_id(&self, build_id: &str, id: &str) -> Result<Option<EntityRow>, GelError> {
        // The id is known from the argument; select the remaining columns.
        let q = format!(
            "match (build: $b, member: $e) isa node-membership; $b isa graph-build, has build-id {}; $e isa code-entity, has entity-id {}, has kind $kind, has repo-name $repo, has snapshot-id $snap, has file $file, has name $name, has qualified-name $qn; select $kind, $repo, $snap, $file, $name, $qn;",
            str_lit(build_id),
            str_lit(id)
        );
        let rows = read_rows(
            self.driver()?,
            &self.config.database,
            &q,
            &["kind", "repo", "snap", "file", "name", "qn"],
        )
        .await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(EntityRow {
            entity_id: id.to_owned(),
            kind: col_string(&row, "kind")?,
            repo: col_string(&row, "repo")?,
            snapshot: col_string(&row, "snap")?,
            file: col_string(&row, "file")?,
            name: col_string(&row, "name")?,
            qualified_name: col_string(&row, "qn")?,
        }))
    }

    async fn neighbors_out(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError> {
        self.neighbors(build_id, id, rel_types, true).await
    }

    async fn neighbors_in(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
    ) -> Result<Vec<RelRow>, GelError> {
        self.neighbors(build_id, id, rel_types, false).await
    }

    async fn build_entities(&self, build_id: &str, limit: i64) -> Result<Vec<EntityRow>, GelError> {
        self.members(build_id, Some(limit)).await
    }

    async fn build_relationships(
        &self,
        build_id: &str,
        limit: i64,
    ) -> Result<Vec<RelRow>, GelError> {
        self.member_rels(build_id, Some(limit)).await
    }

    async fn evidence_for(
        &self,
        build_id: &str,
        rel_id: &str,
    ) -> Result<Vec<EvidenceRow>, GelError> {
        // Claims about this relationship, gated on its membership in the
        // pinned build; supporting and contradicting links union below.
        let mut out: BTreeMap<String, EvidenceRow> = BTreeMap::new();
        for link in ["supporting", "contradicting"] {
            let q = format!(
                "match (build: $b, edge: $rel) isa edge-membership; $b isa graph-build, has build-id {}; $rel isa relationship, has rel-id {}; $c isa claim, has relationship-id {}; (claim: $c, evidence: $e) isa {link}; $e isa evidence, has evidence-id $id, has class $cl, has supports $s, has text $t; select $id, $cl, $s, $t;",
                str_lit(build_id),
                str_lit(rel_id),
                str_lit(rel_id)
            );
            let rows = read_rows(
                self.driver()?,
                &self.config.database,
                &q,
                &["id", "cl", "s", "t"],
            )
            .await?;
            for row in &rows {
                let ev = EvidenceRow {
                    evidence_id: col_string(row, "id")?,
                    class: col_string(row, "cl")?,
                    supports: col_bool(row, "s")?,
                    text: col_string(row, "t")?,
                };
                out.insert(ev.evidence_id.clone(), ev);
            }
        }
        let mut v: Vec<EvidenceRow> = out.into_values().collect();
        v.sort_by(|a, b| a.evidence_id.cmp(&b.evidence_id));
        Ok(v)
    }
}

impl TypeDbReader {
    /// Directed neighborhood: a single membership-scoped query bucketed by
    /// relation type in Rust. An empty filter matches nothing.
    async fn neighbors(
        &self,
        build_id: &str,
        id: &str,
        rel_types: Vec<String>,
        outgoing: bool,
    ) -> Result<Vec<RelRow>, GelError> {
        if rel_types.is_empty() {
            return Ok(Vec::new());
        }
        let endpoint = if outgoing {
            format!("$f isa code-entity, has entity-id {}", str_lit(id))
        } else {
            format!("$t isa code-entity, has entity-id {}", str_lit(id))
        };
        let q = format!(
            "match (build: $b, edge: $rel) isa edge-membership; $b isa graph-build, has build-id {}; $rel isa relationship (from-entity: $f, to-entity: $t), has rel-id $r, has rel-type $rt; {endpoint}; $f isa code-entity, has entity-id $fid; $t isa code-entity, has entity-id $tid; select $r, $rt, $fid, $tid; sort $r;",
            str_lit(build_id)
        );
        // TypeQL needs the anchored endpoint named explicitly; the other
        // endpoint binds through the relationship players.
        let rows = read_rows(self.driver()?, &self.config.database, &q, REL_COLS).await?;
        let mut out: Vec<RelRow> = Vec::new();
        for row in &rows {
            let row_type = col_string(row, "rt")?;
            if !rel_types.contains(&row_type) {
                continue;
            }
            out.push(RelRow {
                rel_id: col_string(row, "r")?,
                rel_type: row_type,
                from_entity: EndpointRef {
                    entity_id: col_string(row, "fid")?,
                },
                to_entity: EndpointRef {
                    entity_id: col_string(row, "tid")?,
                },
            });
        }
        out.sort_by(|a, b| a.rel_id.cmp(&b.rel_id));
        Ok(out)
    }
}
