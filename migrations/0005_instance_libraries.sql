CREATE TABLE IF NOT EXISTS instances_libraries(
    instance_id INTEGER NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    library_id  INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,

    PRIMARY KEY (instance_id, library_id)
);
