-- Add migration script here
ALTER TABLE instances_overview
    RENAME COLUMN "changed_name" TO "name";

ALTER TABLE instances
    DROP COLUMN name;
