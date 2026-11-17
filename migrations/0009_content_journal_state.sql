-- Only recovery_json carries a publication plan. plan_json was always empty.
ALTER TABLE content_operations DROP COLUMN plan_json;

CREATE TRIGGER content_operations_insert_state
BEFORE INSERT ON content_operations
FOR EACH ROW WHEN NEW.state <> 'publishing'
BEGIN
    SELECT RAISE(ABORT, 'content operation must start in publishing');
END;

CREATE TRIGGER content_operations_update_state
BEFORE UPDATE OF state ON content_operations
FOR EACH ROW
WHEN NEW.state <> OLD.state
 AND (OLD.state <> 'publishing' OR NEW.state <> 'completed')
BEGIN
    SELECT RAISE(ABORT, 'invalid content operation state transition');
END;
