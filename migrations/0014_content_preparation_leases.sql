ALTER TABLE content_operations ADD COLUMN store_root BLOB;

DROP TRIGGER content_operations_insert_state;
DROP TRIGGER content_operations_update_state;

CREATE TRIGGER content_operations_insert_state
BEFORE INSERT ON content_operations
WHEN NEW.state NOT IN ('preparing', 'publishing')
  OR (NEW.state = 'preparing' AND NEW.store_root IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'content operation must start in preparing or legacy publishing');
END;

CREATE TRIGGER content_operations_update_state
BEFORE UPDATE OF state ON content_operations
WHEN NEW.state <> OLD.state
 AND NOT ((OLD.state = 'preparing' AND NEW.state = 'publishing')
       OR (OLD.state = 'publishing' AND NEW.state = 'completed'))
BEGIN
    SELECT RAISE(ABORT, 'invalid content operation state transition');
END;
