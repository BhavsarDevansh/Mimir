-- Add rejected_action predicate for threshold inference rule (Issue #54)
INSERT INTO predicates (id, name, description) VALUES
    (12, 'rejected_action', 'Subject rejected performing action');

-- Constraints: Person -> Activity
INSERT INTO predicate_constraints (predicate_id, allowed_subject_type_id, allowed_object_type_id) VALUES
    (12, 1, 7);
