-- no-transaction
-- ============================================================================
-- 036: Seed relationship type aliases: self-aliases + legacy hardcoded synonyms
-- ============================================================================
PRAGMA foreign_keys = OFF;

-- 1. Self-aliases for every existing canonical relationship type.
--    This makes the alias table the single source of truth for resolution.
INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id)
SELECT name, id FROM relationship_types;

-- 2. Legacy hardcoded synonyms from extract.rs::normalize_predicate.
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
