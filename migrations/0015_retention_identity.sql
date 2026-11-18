-- Persistent identity detects a database replaced at an already catalogued path.
CREATE TABLE retention_identity (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    identity TEXT NOT NULL CHECK(length(identity) = 32 AND identity NOT GLOB '*[^0-9a-f]*')
);
INSERT INTO retention_identity VALUES(1, lower(hex(randomblob(16))));
CREATE TRIGGER retention_identity_update BEFORE UPDATE ON retention_identity
BEGIN SELECT RAISE(ABORT, 'retention identity is immutable'); END;
CREATE TRIGGER retention_identity_delete BEFORE DELETE ON retention_identity
BEGIN SELECT RAISE(ABORT, 'retention identity is immutable'); END;
