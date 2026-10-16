//! Stateless repository-level neighborhood pages. Scoped read-view cursors and
//! their connector-owned admission/evidence validation are a separate contract.

use chaosbox_store::{GraphQueries, NeighborhoodDirection, RelRow};
use serde::{Deserialize, Serialize};

use super::{GraphReader, PipelineError, all_relation_types, validate_rel_filter};
use crate::read_view::bounded_json;

const CONTRACT: &str = "chaosbox.repository-neighborhood";
const ORDER: &str = "rel_id_asc";
const VERSION: u32 = 1;
// Match the existing scoped-reader page and default request-size ceilings.
const PAGE_MAX: usize = 200;
const REQUEST_BYTES_MAX: usize = 16 * 1024;

/// Directions delivered by one page; self-loops belong to each selected direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeighborhoodMode {
    /// Deliver outgoing relationships only.
    Outgoing,
    /// Deliver incoming relationships only.
    Incoming,
    /// Retain the ordinary neighborhood's two directional arrays.
    #[default]
    Both,
}

impl std::str::FromStr for NeighborhoodMode {
    type Err = PipelineError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(value.into()))
            .map_err(|_| invalid("direction: expected both, outgoing or incoming"))
    }
}

/// An ordinary repository read, not an authorization decision. Callers must
/// confine the backend/repository they expose. A cursor grants no access and
/// cannot be used in place of a connector-authorized scoped read view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeighborhoodQuery {
    /// Explicit repository, at most 256 bytes; ownership is checked on every page.
    pub repo: String,
    /// Anchor entity id, at most 256 bytes.
    pub id: String,
    /// Selected directions; defaults to both.
    #[serde(default)]
    pub direction: NeighborhoodMode,
    /// Case-insensitive relation names. None means all; an empty list means none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel_types: Option<Vec<String>>,
    /// Delivered rows per selected direction, in 1..=200; bound into the cursor.
    pub limit: usize,
    /// Opaque continuation from a previous response, never a credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// One bounded neighborhood response, pinned to exactly one publication.
#[derive(Debug, Serialize)]
pub struct NeighborhoodPage {
    /// Page contract version.
    pub version: u32,
    /// Repository owning the pinned publication.
    pub repo: String,
    /// Anchor entity id.
    pub id: String,
    /// Directions selected by the request.
    pub direction: NeighborhoodMode,
    /// Exact publication used, including on resumed historical reads.
    pub build_id: String,
    /// Generation recorded in that publication's header.
    pub generation: i64,
    /// Outgoing rows, ordered by relationship id.
    pub outgoing: Vec<RelRow>,
    /// Incoming rows, ordered by relationship id.
    pub incoming: Vec<RelRow>,
    /// No outgoing rows remain; also true when that direction was not requested.
    pub outgoing_complete: bool,
    /// No incoming rows remain; also true when that direction was not requested.
    pub incoming_complete: bool,
    /// Both selected directions have been exhausted.
    pub complete: bool,
    /// Resume both positions against the same immutable request and publication.
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Position {
    Start,
    After(String),
    Done,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    contract: String,
    version: u32,
    order: String,
    scope: NeighborhoodQuery,
    build_id: String,
    outgoing: Position,
    incoming: Position,
}

fn invalid(what: &str) -> PipelineError {
    PipelineError::Consumer(format!("invalid neighborhood {what}"))
}

fn valid_label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

impl NeighborhoodQuery {
    /// Validate and canonicalize before callers connect or load credentials.
    /// The result contains no authorization grant or backend handle.
    pub fn prepare(mut self) -> Result<PreparedNeighborhood, PipelineError> {
        if self
            .cursor
            .as_ref()
            .is_some_and(|c| c.len() > REQUEST_BYTES_MAX)
        {
            return Err(invalid("cursor"));
        }
        bounded_json(&self, REQUEST_BYTES_MAX).map_err(|_| invalid("request"))?;
        if !valid_label(&self.repo)
            || !valid_label(&self.id)
            || !(1..=PAGE_MAX).contains(&self.limit)
        {
            return Err(invalid("arguments"));
        }
        let mut kinds =
            validate_rel_filter(self.rel_types.take())?.unwrap_or_else(all_relation_types);
        kinds.sort();
        kinds.dedup();
        self.rel_types = Some(kinds);
        let cursor = self
            .cursor
            .take()
            .map(|raw| {
                let cursor: Cursor = serde_json::from_str(&raw).map_err(|_| invalid("cursor"))?;
                let positions_valid =
                    [&cursor.outgoing, &cursor.incoming]
                        .iter()
                        .all(|p| match p {
                            Position::After(key) => valid_label(key),
                            Position::Done => true,
                            Position::Start => false,
                        });
                if cursor.contract != CONTRACT
                    || cursor.version != VERSION
                    || cursor.order != ORDER
                    || cursor.scope != self
                    || !valid_label(&cursor.build_id)
                    || !positions_valid
                    || matches!(
                        (&cursor.outgoing, &cursor.incoming),
                        (Position::Done, Position::Done)
                    )
                    || (self.direction == NeighborhoodMode::Incoming
                        && !matches!(cursor.outgoing, Position::Done))
                    || (self.direction == NeighborhoodMode::Outgoing
                        && !matches!(cursor.incoming, Position::Done))
                {
                    return Err(invalid("cursor"));
                }
                Ok(cursor)
            })
            .transpose()?;
        Ok(PreparedNeighborhood {
            query: self,
            cursor,
        })
    }
}

/// Validated query state, prepared before backend/credential acquisition.
pub struct PreparedNeighborhood {
    query: NeighborhoodQuery,
    cursor: Option<Cursor>,
}

impl PreparedNeighborhood {
    /// Revalidate publication ownership and read the page. Continuations resolve
    /// `published_build` without an active-pointer fallback. The cursor is
    /// untrusted query state, not authenticated server issuance.
    pub async fn read<R: GraphQueries>(self, handle: R) -> Result<NeighborhoodPage, PipelineError> {
        let Self { query, cursor } = self;
        let reader = match &cursor {
            Some(cursor) => GraphReader::published(handle, &query.repo, &cursor.build_id).await?,
            None => GraphReader::pinned(handle, &query.repo).await?,
        };
        let mut cursor = cursor.unwrap_or_else(|| Cursor {
            contract: CONTRACT.into(),
            version: VERSION,
            order: ORDER.into(),
            scope: query.clone(),
            build_id: reader.build_id.clone(),
            outgoing: if query.direction == NeighborhoodMode::Incoming {
                Position::Done
            } else {
                Position::Start
            },
            incoming: if query.direction == NeighborhoodMode::Outgoing {
                Position::Done
            } else {
                Position::Start
            },
        });
        let outgoing = reader
            .directional_page(
                &query,
                NeighborhoodDirection::Outgoing,
                &mut cursor.outgoing,
            )
            .await?;
        let incoming = reader
            .directional_page(
                &query,
                NeighborhoodDirection::Incoming,
                &mut cursor.incoming,
            )
            .await?;
        let outgoing_complete = matches!(cursor.outgoing, Position::Done);
        let incoming_complete = matches!(cursor.incoming, Position::Done);
        let complete = outgoing_complete && incoming_complete;
        let next_cursor = if complete {
            None
        } else {
            let bytes = bounded_json(&cursor, REQUEST_BYTES_MAX).map_err(|_| invalid("cursor"))?;
            Some(String::from_utf8(bytes).map_err(|_| invalid("cursor"))?)
        };
        Ok(NeighborhoodPage {
            version: VERSION,
            repo: query.repo,
            id: query.id,
            direction: query.direction,
            build_id: reader.build_id,
            generation: reader.generation,
            outgoing,
            incoming,
            outgoing_complete,
            incoming_complete,
            complete,
            next_cursor,
        })
    }
}

impl<R: GraphQueries> GraphReader<R> {
    /// Validate and read on an existing handle. Transports that acquire a backend
    /// should call `NeighborhoodQuery::prepare` before connecting instead.
    pub async fn neighborhood_page(
        handle: R,
        query: NeighborhoodQuery,
    ) -> Result<NeighborhoodPage, PipelineError> {
        query.prepare()?.read(handle).await
    }

    async fn directional_page(
        &self,
        query: &NeighborhoodQuery,
        direction: NeighborhoodDirection,
        position: &mut Position,
    ) -> Result<Vec<RelRow>, PipelineError> {
        let kinds = query.rel_types.clone().unwrap_or_default();
        if matches!(position, Position::Done) || kinds.is_empty() {
            *position = Position::Done;
            return Ok(Vec::new());
        }
        let after = match position {
            Position::After(key) => Some(key.as_str()),
            _ => None,
        };
        let limit = i64::try_from(query.limit + 1).map_err(|_| invalid("limit"))?;
        let mut rows = self
            .handle
            .neighbor_page(&self.build_id, &query.id, direction, kinds, after, limit)
            .await
            .map_err(|e| PipelineError::Consumer(e.to_string()))?;
        if rows.len() > query.limit + 1 {
            return Err(PipelineError::Consumer(
                "backend neighborhood page exceeded limit".into(),
            ));
        }
        let more = rows.len() > query.limit;
        rows.truncate(query.limit);
        *position = if more {
            Position::After(rows.last().ok_or_else(|| invalid("page"))?.rel_id.clone())
        } else {
            Position::Done
        };
        Ok(rows)
    }
}
