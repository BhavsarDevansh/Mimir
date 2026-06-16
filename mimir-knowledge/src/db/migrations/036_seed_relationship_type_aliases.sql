-- no-transaction
-- ============================================================================
-- 036: Seed relationship type aliases: self-aliases + legacy hardcoded synonyms
-- ============================================================================
PRAGMA foreign_keys = OFF;

-- 1. Ensure the canonical targets referenced by legacy hardcoded synonyms exist.
--    INSERT OR IGNORE keeps existing rows and only creates missing ones.
INSERT OR IGNORE INTO relationship_types (name, description) VALUES
    ('studied_at', 'Subject studied at or attended an institution'),
    ('hobby', 'Subject has a hobby or interest'),
    ('works_at', 'Subject works at or is employed by an organization'),
    ('works_as', 'Subject works in a particular role or profession'),
    ('based_in', 'Subject is currently based in or resides in a location'),
    ('lived_in', 'Subject previously lived in a location'),
    ('has_pets', 'Subject has pets'),
    ('has_sibling', 'Subject has a sibling'),
    ('has_partner', 'Subject has a partner or spouse'),
    ('has_parent', 'Subject has a parent'),
    ('has_child', 'Subject has a child'),
    ('preferred_name', 'Subject prefers to be called by a particular name'),
    ('favourite_food', 'Subject''s favourite food'),
    ('favourite_colour', 'Subject''s favourite colour'),
    ('health_condition', 'Subject has a health condition, allergy, or medical condition');

-- 2. Self-aliases for every canonical relationship type.
--    This makes the alias table the single source of truth for resolution.
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT name, id FROM relationship_types;

-- 3. Legacy hardcoded synonyms from extract.rs::normalize_predicate.
--    Each resolves to a canonical relationship type already in relationship_types.
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'attended', id FROM relationship_types WHERE name = 'studied_at';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'went_to', id FROM relationship_types WHERE name = 'studied_at';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'graduated_from', id FROM relationship_types WHERE name = 'studied_at';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'alumni_of', id FROM relationship_types WHERE name = 'studied_at';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'hobbies', id FROM relationship_types WHERE name = 'hobby';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'interests', id FROM relationship_types WHERE name = 'hobby';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'works_for', id FROM relationship_types WHERE name = 'works_at';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'employer', id FROM relationship_types WHERE name = 'works_at';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'profession', id FROM relationship_types WHERE name = 'works_as';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'occupation', id FROM relationship_types WHERE name = 'works_as';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'resides_in', id FROM relationship_types WHERE name = 'based_in';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'current_city', id FROM relationship_types WHERE name = 'based_in';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'previously_lived_in', id FROM relationship_types WHERE name = 'lived_in';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'former_city', id FROM relationship_types WHERE name = 'lived_in';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'pet', id FROM relationship_types WHERE name = 'has_pets';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'pets', id FROM relationship_types WHERE name = 'has_pets';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'owns_pet', id FROM relationship_types WHERE name = 'has_pets';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'brother', id FROM relationship_types WHERE name = 'has_sibling';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'sister', id FROM relationship_types WHERE name = 'has_sibling';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'siblings', id FROM relationship_types WHERE name = 'has_sibling';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'spouse', id FROM relationship_types WHERE name = 'has_partner';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'boyfriend', id FROM relationship_types WHERE name = 'has_partner';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'girlfriend', id FROM relationship_types WHERE name = 'has_partner';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'partner', id FROM relationship_types WHERE name = 'has_partner';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'wife', id FROM relationship_types WHERE name = 'has_partner';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'husband', id FROM relationship_types WHERE name = 'has_partner';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'father', id FROM relationship_types WHERE name = 'has_parent';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'mother', id FROM relationship_types WHERE name = 'has_parent';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'parents', id FROM relationship_types WHERE name = 'has_parent';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'son', id FROM relationship_types WHERE name = 'has_child';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'daughter', id FROM relationship_types WHERE name = 'has_child';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'children', id FROM relationship_types WHERE name = 'has_child';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'nickname', id FROM relationship_types WHERE name = 'preferred_name';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'nick_name', id FROM relationship_types WHERE name = 'preferred_name';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'called', id FROM relationship_types WHERE name = 'preferred_name';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'goes_by', id FROM relationship_types WHERE name = 'preferred_name';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'favorite_food', id FROM relationship_types WHERE name = 'favourite_food';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'fav_food', id FROM relationship_types WHERE name = 'favourite_food';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'favorite_colour', id FROM relationship_types WHERE name = 'favourite_colour';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'favorite_color', id FROM relationship_types WHERE name = 'favourite_colour';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'fav_color', id FROM relationship_types WHERE name = 'favourite_colour';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'fav_colour', id FROM relationship_types WHERE name = 'favourite_colour';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'color', id FROM relationship_types WHERE name = 'favourite_colour';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'colour', id FROM relationship_types WHERE name = 'favourite_colour';

INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'food_allergy', id FROM relationship_types WHERE name = 'health_condition';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'medical_condition', id FROM relationship_types WHERE name = 'health_condition';
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT 'condition', id FROM relationship_types WHERE name = 'health_condition';

PRAGMA foreign_keys = ON;
