CREATE TABLE content_objects (
    id INTEGER PRIMARY KEY,
    sha256 TEXT NOT NULL UNIQUE CHECK(length(sha256) = 64),
    size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
    sha1 TEXT,
    sha512 TEXT,
    curseforge_fingerprint INTEGER,
    store_path TEXT UNIQUE,
    store_state TEXT NOT NULL DEFAULT 'external'
        CHECK(store_state IN ('external', 'ready', 'quarantined', 'corrupt')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_verified_at TEXT
);

CREATE TABLE content_metadata (
    object_id INTEGER NOT NULL REFERENCES content_objects(id) ON DELETE CASCADE,
    parser_name TEXT NOT NULL,
    parser_version INTEGER NOT NULL,
    descriptor_path TEXT NOT NULL,
    component_index INTEGER NOT NULL DEFAULT 0,
    metadata_json TEXT NOT NULL,
    PRIMARY KEY(object_id, parser_name, descriptor_path, component_index)
);

CREATE TABLE content_provider_files (
    id INTEGER PRIMARY KEY,
    object_id INTEGER NOT NULL REFERENCES content_objects(id) ON DELETE RESTRICT,
    provider TEXT NOT NULL CHECK(provider IN ('curseforge', 'modrinth')),
    project_id TEXT NOT NULL,
    release_id TEXT NOT NULL,
    file_key TEXT NOT NULL,
    match_evidence TEXT NOT NULL,
    UNIQUE(object_id, provider, project_id, release_id, file_key)
);

CREATE TABLE instance_content (
    id INTEGER PRIMARY KEY,
    instance_id INTEGER NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    object_id INTEGER REFERENCES content_objects(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK(kind IN ('mod', 'resource_pack', 'shader_pack')),
    relative_path TEXT NOT NULL,
    path_key BLOB NOT NULL,
    mod_enabled INTEGER CHECK(mod_enabled IN (0, 1)),
    placement TEXT NOT NULL
        CHECK(placement IN ('external', 'copy', 'hardlink', 'reflink')),
    presence TEXT NOT NULL DEFAULT 'present'
        CHECK(presence IN ('present', 'missing', 'unreadable', 'unstable')),
    observed_size INTEGER,
    observed_modified TEXT,
    observed_identity BLOB,
    indexed_fingerprint BLOB,
    pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN (0, 1)),
    preferred_provider_file_id INTEGER
        REFERENCES content_provider_files(id) ON DELETE SET NULL,
    seen_generation INTEGER NOT NULL DEFAULT 0,
    UNIQUE(instance_id, path_key),
    CHECK((kind = 'mod' AND mod_enabled IS NOT NULL)
        OR (kind <> 'mod' AND mod_enabled IS NULL))
);

CREATE INDEX instance_content_object ON instance_content(object_id);
CREATE INDEX provider_files_object ON content_provider_files(object_id);

CREATE TABLE content_operations (
    id TEXT PRIMARY KEY,
    instance_id INTEGER NOT NULL REFERENCES instances(id) ON DELETE RESTRICT,
    state TEXT NOT NULL,
    plan_json TEXT NOT NULL,
    recovery_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE content_object_leases (
    operation_id TEXT NOT NULL REFERENCES content_operations(id) ON DELETE CASCADE,
    object_id INTEGER NOT NULL REFERENCES content_objects(id) ON DELETE RESTRICT,
    PRIMARY KEY(operation_id, object_id)
);
