-- Bytes identity is not a library's path, role or classpath position.
CREATE TABLE instance_library_placements (
    instance_id INTEGER NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK(position >= 0),
    hash TEXT NOT NULL CHECK(length(hash) = 40),
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    native INTEGER NOT NULL CHECK(native IN (0, 1)),
    url TEXT NOT NULL,
    PRIMARY KEY(instance_id, position)
);

INSERT INTO instance_library_placements(instance_id,position,hash,name,path,native,url)
SELECT il.instance_id, il.position, l.hash, l.name, l.path, l.native, l.url
FROM instances_libraries il JOIN libraries l ON l.id = il.library_id;

-- Keep legacy tables as an upgrade snapshot, not as the source of new writes.
