CREATE TABLE IF NOT EXISTS incidents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    impact TEXT NOT NULL DEFAULT 'minor',      -- none / minor / major / critical（Statuspage 的四级）
    components TEXT NOT NULL DEFAULT '[]',     -- JSON 数组，取值见 COMPONENTS
    started_at REAL NOT NULL,
    resolved_at REAL
);
CREATE TABLE IF NOT EXISTS incident_updates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    incident_id INTEGER NOT NULL,
    status TEXT NOT NULL,                      -- investigating / identified / update / monitoring / resolved
    body TEXT NOT NULL,
    at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_incident_updates ON incident_updates(incident_id, at);
