CREATE TABLE IF NOT EXISTS instances(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    dir TEXT NOT NULL UNIQUE,
    version TEXT NOT NULL,
    loader TEXT NOT NULL,
    manifest_url TEXT NOT NULL,
    meta_provider TEXT NOT NULL,
    created_at DATETIME DEFAULT (datetime('now')),
    updated_at DATETIME DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS instances_overview(
    instance_id INTEGER PRIMARY KEY REFERENCES instances(id) ON DELETE CASCADE,
    tags TEXT NOT NULL DEFAULT '',
    export_type TEXT NOT NULL DEFAULT 'Sonata',
    playtime INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS instances_settings(
    instance_id INTEGER PRIMARY KEY REFERENCES instances(id) ON DELETE CASCADE,
    java_runtime INTEGER REFERENCES java_runtimes(id) ON DELETE SET NULL
);
