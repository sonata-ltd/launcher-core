-- Add migration script here
CREATE TABLE IF NOT EXISTS instances(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    loader TEXT NOT NULL,
    created_at DATETIME DEFAULT (datetime('now')),
    updated_at DATETIME DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS instances_overview(
    instance_id INTEGER PRIMARY KEY REFERENCES instances(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    tags TEXT NOT NULL,
    export_type TEXT NOT NULL DEFAULT "Sonata",
    playtime INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS instances_settings(
    instance_id INTEGER PRIMARY KEY REFERENCES instances(id) ON DELETE CASCADE,
    dir TEXT NOT NULL
);
