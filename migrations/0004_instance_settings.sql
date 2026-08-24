ALTER TABLE instances_settings ADD COLUMN memory_min INTEGER;
ALTER TABLE instances_settings ADD COLUMN memory_max INTEGER;
ALTER TABLE instances_settings ADD COLUMN jvm_args   TEXT;

CREATE TABLE IF NOT EXISTS settings_global(
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    java_runtime INTEGER REFERENCES java_runtimes(id) ON DELETE SET NULL,
    memory_min   INTEGER NOT NULL DEFAULT 512,
    memory_max   INTEGER NOT NULL DEFAULT 4096,
    jvm_args     TEXT    NOT NULL DEFAULT ''
);

INSERT OR IGNORE INTO settings_global (id) VALUES (1);
