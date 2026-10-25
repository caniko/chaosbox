-- Chaosbox PostgreSQL schema. `chaosbox db migrate` applies this file in one
-- transaction under an advisory lock; every statement is idempotent and
-- changes are additive. Payloads are `text`, never jsonb: digests, claims,
-- replica bodies and sealed evidence are compared byte for byte. Key and sort
-- columns use the "C" collation so SQL order equals Rust byte order.

-- pgvector is not a trusted extension: an operator (superuser) creates it
-- once; this statement only verifies it when it already exists.
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS schema_version (
  version integer PRIMARY KEY
);

-- Immutable published builds. A row exists only once its members committed.
CREATE TABLE IF NOT EXISTS graph_builds (
  build_id text COLLATE "C" PRIMARY KEY,
  repo text COLLATE "C" NOT NULL,
  generation bigint NOT NULL,
  predecessor text,
  snapshot_ids text[] NOT NULL,
  coverage text,
  navigation text NOT NULL,
  digest text NOT NULL,
  node_count bigint NOT NULL,
  edge_count bigint NOT NULL,
  created timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS graph_builds_repo ON graph_builds (repo, build_id);

CREATE TABLE IF NOT EXISTS active_pointer (
  repo text COLLATE "C" PRIMARY KEY,
  build_id text COLLATE "C" NOT NULL REFERENCES graph_builds,
  updated timestamptz NOT NULL DEFAULT now()
);

-- Entity ids are per snapshot and shared by every build of that snapshot.
CREATE TABLE IF NOT EXISTS entities (
  entity_id text COLLATE "C" PRIMARY KEY,
  kind text NOT NULL,
  repo text NOT NULL,
  snapshot_id text NOT NULL,
  file text NOT NULL,
  name text NOT NULL,
  qualified_name text COLLATE "C" NOT NULL,
  name_fold text NOT NULL,
  qn_fold text NOT NULL,
  span text NOT NULL,
  compiler text,
  text_sha256 text
);

CREATE TABLE IF NOT EXISTS node_members (
  build_id text COLLATE "C" NOT NULL REFERENCES graph_builds,
  entity_id text COLLATE "C" NOT NULL REFERENCES entities,
  PRIMARY KEY (build_id, entity_id)
);

-- Relationship ids already hash their build, so each row is one membership.
CREATE TABLE IF NOT EXISTS relationships (
  build_id text COLLATE "C" NOT NULL REFERENCES graph_builds,
  rel_id text COLLATE "C" NOT NULL,
  rel_type text NOT NULL,
  from_id text COLLATE "C" NOT NULL,
  to_id text COLLATE "C" NOT NULL,
  scope text NOT NULL,
  sealed_evidence text NOT NULL,
  PRIMARY KEY (build_id, rel_id)
);
CREATE INDEX IF NOT EXISTS relationships_from ON relationships (build_id, from_id, rel_id);
CREATE INDEX IF NOT EXISTS relationships_to ON relationships (build_id, to_id, rel_id);

-- Audit rows behind publication: keyed JSON bodies.
CREATE TABLE IF NOT EXISTS runs (run_id text PRIMARY KEY, body text NOT NULL);
CREATE TABLE IF NOT EXISTS candidate_sets (set_id text PRIMARY KEY, body text NOT NULL);
CREATE TABLE IF NOT EXISTS candidates (candidate_id text PRIMARY KEY, body text NOT NULL);
CREATE TABLE IF NOT EXISTS file_versions (
  snapshot_id text NOT NULL,
  path text NOT NULL,
  sha256 text NOT NULL,
  bytes bigint NOT NULL,
  PRIMARY KEY (snapshot_id, path)
);
CREATE TABLE IF NOT EXISTS evidence (evidence_id text PRIMARY KEY, body text NOT NULL);
CREATE TABLE IF NOT EXISTS claims (claim_id text PRIMARY KEY, body text NOT NULL);

-- Durable-first paid answers: one decision per (candidate, question), one
-- inference per reuse key (first write wins).
CREATE TABLE IF NOT EXISTS decisions (
  decision_key text PRIMARY KEY,
  cache_key text NOT NULL,
  failed boolean NOT NULL,
  body text NOT NULL
);
CREATE TABLE IF NOT EXISTS inferences (reuse_key text PRIMARY KEY, body text NOT NULL);

-- Private session knowledge generations and their per-scope pointer.
CREATE TABLE IF NOT EXISTS knowledge_builds (
  build_id text PRIMARY KEY,
  scope text NOT NULL,
  body text NOT NULL
);
CREATE TABLE IF NOT EXISTS knowledge_pointer (
  scope text PRIMARY KEY,
  build_id text NOT NULL REFERENCES knowledge_builds
);

-- Same-user peer replication: append-only events and a published view.
CREATE TABLE IF NOT EXISTS replica_events (
  replica_id text COLLATE "C" PRIMARY KEY,
  replica_user text NOT NULL,
  scope text NOT NULL,
  body text NOT NULL
);
CREATE INDEX IF NOT EXISTS replica_events_page ON replica_events (replica_user, scope, replica_id);
CREATE TABLE IF NOT EXISTS replica_views (
  view_id text PRIMARY KEY,
  replica_user text NOT NULL,
  scope text NOT NULL,
  body text NOT NULL
);
CREATE TABLE IF NOT EXISTS replica_pointer (
  replica_user text NOT NULL,
  scope text NOT NULL,
  view_id text NOT NULL REFERENCES replica_views,
  PRIMARY KEY (replica_user, scope)
);

-- Content-addressed embedding inputs (entity declarations, knowledge
-- statements) and their derived vectors. Vectors never enter publication.
CREATE TABLE IF NOT EXISTS texts (
  text_sha256 text PRIMARY KEY,
  body text NOT NULL
);
CREATE TABLE IF NOT EXISTS embeddings (
  model text NOT NULL,
  text_sha256 text NOT NULL REFERENCES texts,
  vec halfvec(1024) NOT NULL,
  PRIMARY KEY (model, text_sha256)
);

INSERT INTO schema_version (version) VALUES (1) ON CONFLICT DO NOTHING;
