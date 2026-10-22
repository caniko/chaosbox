CREATE TABLE IF NOT EXISTS identity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    version INTEGER NOT NULL CHECK (version = 1),
    scope TEXT NOT NULL, host TEXT NOT NULL, store TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS operations (
    id TEXT PRIMARY KEY, digest TEXT NOT NULL, repo TEXT NOT NULL,
    reason TEXT NOT NULL, path TEXT NOT NULL, request TEXT NOT NULL,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS settlements (
    id TEXT PRIMARY KEY REFERENCES operations(id), receipt TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS executions (
    id TEXT PRIMARY KEY REFERENCES operations(id), receipt TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS operations_repo ON operations(repo);
