-- no-transaction
-- ============================================================================
-- 031: Category taxonomy + rename predicates → relationship_types
-- ============================================================================
PRAGMA foreign_keys = OFF;

-- ============================================================================
-- 1. Rename predicates → relationship_types
-- ============================================================================

-- 1a. Create new relationship_types table
CREATE TABLE relationship_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    sensitive BOOLEAN NOT NULL DEFAULT FALSE
);

-- 1b. Copy data from predicates
INSERT INTO relationship_types (id, name, description, sensitive)
SELECT id, name, description, COALESCE(sensitive, FALSE) FROM predicates;

-- 1c. Create new relationship_constraints
CREATE TABLE relationship_constraints (
    relationship_type_id INTEGER NOT NULL REFERENCES relationship_types(id) ON DELETE CASCADE,
    allowed_subject_type_id INTEGER NOT NULL REFERENCES entity_types(id),
    allowed_object_type_id INTEGER NOT NULL REFERENCES entity_types(id),
    PRIMARY KEY (relationship_type_id, allowed_subject_type_id, allowed_object_type_id)
);

-- 1d. Copy constraints data (predicate_id → relationship_type_id)
INSERT INTO relationship_constraints (relationship_type_id, allowed_subject_type_id, allowed_object_type_id)
SELECT predicate_id, allowed_subject_type_id, allowed_object_type_id FROM predicate_constraints;

-- 1e. Drop old predicate_constraints
DROP TABLE predicate_constraints;

-- 1f. Drop old predicates table
DROP TABLE predicates;

-- ============================================================================
-- 2. Rename predicate_id → relationship_type_id in facts
-- ============================================================================

CREATE TABLE facts_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_id INTEGER NOT NULL REFERENCES entities(id),
    relationship_type_id INTEGER NOT NULL REFERENCES relationship_types(id),
    object_id INTEGER REFERENCES entities(id),
    object_literal TEXT,
    valid_from TIMESTAMP,
    valid_until TIMESTAMP,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    fact_status_id INTEGER NOT NULL DEFAULT 1 REFERENCES fact_statuses(id),
    inferred BOOLEAN NOT NULL DEFAULT FALSE,
    inference_depth INTEGER NOT NULL DEFAULT 0,
    stale_confidence BOOLEAN NOT NULL DEFAULT FALSE,
    pending_confirmation BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO facts_new (
    id, subject_id, relationship_type_id, object_id, object_literal,
    valid_from, valid_until, confidence, fact_status_id, inferred,
    inference_depth, stale_confidence, pending_confirmation, created_at, updated_at
)
SELECT
    id, subject_id, predicate_id, object_id, object_literal,
    valid_from, valid_until, confidence, fact_status_id, inferred,
    inference_depth, stale_confidence, pending_confirmation, created_at, updated_at
FROM facts;

DROP TABLE facts;
ALTER TABLE facts_new RENAME TO facts;

-- Recreate fact indexes
CREATE INDEX idx_facts_subject ON facts(subject_id);
CREATE INDEX idx_facts_object ON facts(object_id);
CREATE INDEX idx_facts_relationship ON facts(relationship_type_id);
CREATE INDEX idx_facts_status ON facts(fact_status_id);
CREATE INDEX idx_facts_temporal ON facts(valid_from, valid_until);

-- ============================================================================
-- 3. Create categories table (Dewey Decimal-style)
-- ============================================================================

CREATE TABLE categories (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    parent_id INTEGER REFERENCES categories(id),
    memory_weight REAL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_categories_parent ON categories(parent_id);

-- ============================================================================
-- 4. Create fact_categories junction table
-- ============================================================================

CREATE TABLE fact_categories (
    fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    category_id INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    PRIMARY KEY (fact_id, category_id)
);

CREATE INDEX idx_fact_categories_fact ON fact_categories(fact_id);
CREATE INDEX idx_fact_categories_category ON fact_categories(category_id);

-- ============================================================================
-- 5. Seed categories
-- ============================================================================

INSERT INTO categories (id, name, description, parent_id, memory_weight) VALUES
(0,   'Root', 'Top-level root of the category taxonomy', NULL, NULL),
(1,   'System & Meta', 'Mimir system configuration, agent behaviour, and knowledge graph rules', NULL, 0.0),
(10,  'Mimir Configuration', 'User-configured settings, preferences, and defaults', 1, 0.0),
(11,  'Knowledge Graph Structure', 'Schema, rules, and structural facts about the KG itself', 1, 0.0),
(12,  'Agent Behaviour', 'Communication style, personality, and interaction preferences', 1, 0.0),
(100, 'Identity & Biography', 'Who the user is — identifiers, origins, values, and life story', NULL, 1.00),
(110, 'Name & Aliases', 'Legal name, nicknames, handles, and how the user prefers to be called', 100, 1.00),
(120, 'Birth & Origins', 'Date and place of birth, nationality, ethnicity, and cultural background', 100, 1.00),
(130, 'Physical Characteristics', 'Height, appearance, distinguishing features, and physical abilities', 100, 0.90),
(140, 'Personal History', 'Major life events, upbringing, education timeline, and formative experiences', 100, 0.85),
(150, 'Cultural & Religious', 'Faith, traditions, holidays observed, and cultural affiliations', 100, 0.80),
(160, 'Languages & Communication', 'Languages spoken, fluency levels, and communication preferences', 100, 0.85),
(170, 'Privacy & Security', 'Boundaries around sharing, sensitive topics, and consent preferences', 100, 0.90),
(180, 'Values & Philosophy', 'Core beliefs, political stance, ethical principles, and worldview', 100, 0.80),
(200, 'Food & Drink', 'Everything the user eats, drinks, prefers, avoids, and enjoys', NULL, 0.90),
(210, 'Tastes & Favourites', 'Foods and dishes the user actively enjoys', 200, 0.90),
(211, 'Sweet Foods', 'Desserts, confectionery, and sweet treats', 210, 0.90),
(212, 'Savoury Foods', 'Meals, snacks, and savoury dishes', 210, 0.90),
(220, 'Aversions & Dislikes', 'Foods and flavours the user dislikes or avoids', 200, 0.85),
(230, 'Allergies & Intolerances', 'Medical food reactions and dietary restrictions for health', 200, 0.95),
(240, 'Dietary Choices', 'Voluntary diets: vegetarian, vegan, halal, kosher, etc.', 200, 0.90),
(250, 'Cuisine Preferences', 'Favourite regional or national cuisines', 200, 0.85),
(260, 'Beverages', 'Drinks: coffee, tea, alcohol, soft drinks, water preferences', 200, 0.85),
(270, 'Cooking & Recipes', 'Skills, techniques, go-to recipes, and kitchen habits', 200, 0.80),
(280, 'Dining Out', 'Favourite restaurants, takeaway habits, and dining preferences', 200, 0.75),
(290, 'Food Shopping', 'Preferred brands, supermarkets, and grocery habits', 200, 0.70),
(300, 'Health & Wellness', 'Medical, fitness, mental health, and wellbeing information', NULL, 0.80),
(310, 'Medical History', 'Past diagnoses, surgeries, and significant health events', 300, 0.95),
(320, 'Current Conditions', 'Ongoing health conditions and their status', 300, 0.95),
(330, 'Medications & Treatments', 'Prescriptions, therapies, and treatment regimens', 300, 0.95),
(340, 'Healthcare Providers', 'Doctors, hospitals, insurance, and medical contacts', 300, 0.85),
(350, 'Fitness & Exercise', 'Workout routines, sports played, and physical activity', 300, 0.80),
(360, 'Sleep & Recovery', 'Sleep schedule, quality, and rest habits', 300, 0.75),
(370, 'Mental Health', 'Therapy, diagnoses, coping strategies, and emotional wellbeing', 300, 0.90),
(380, 'Diet & Nutrition', 'Supplements, macros, and nutritional goals', 300, 0.80),
(390, 'Disabilities & Accessibility', 'Physical or cognitive needs and accommodations', 300, 0.90),
(400, 'Relationships & Social', 'People the user knows and how they relate to them', NULL, 0.85),
(410, 'Family', 'Parents, siblings, children, and extended family', 400, 0.90),
(420, 'Romantic', 'Partner, spouse, dating history, and relationship status', 400, 0.90),
(430, 'Friends', 'Close friends and social circle', 400, 0.85),
(440, 'Pets & Animals', 'Pets owned, animal preferences, and connections to animals', 400, 0.80),
(450, 'Professional Network', 'Colleagues, mentors, clients, and work contacts', 400, 0.75),
(460, 'Social Preferences', 'Introversion, group sizes, and social boundaries', 400, 0.80),
(470, 'Conflicts & Boundaries', 'People or situations to avoid, hard limits', 400, 0.85),
(480, 'Communication Preferences', 'Preferred channels, response times, and interaction style', 400, 0.80),
(500, 'Work & Education', 'Career, skills, qualifications, and professional life', NULL, 0.60),
(510, 'Current Role', 'Current job title, employer, and responsibilities', 500, 0.70),
(520, 'Employment History', 'Previous jobs, employers, and career timeline', 500, 0.60),
(530, 'Career Goals', 'Aspirations, desired roles, and professional ambitions', 500, 0.65),
(540, 'Skills & Expertise', 'Technical, soft, and domain-specific competencies', 500, 0.70),
(550, 'Education', 'Degrees, institutions, and academic background', 500, 0.65),
(560, 'Certifications', 'Professional certifications, licenses, and training', 500, 0.65),
(570, 'Work Preferences', 'Remote vs office, hours, environment, and culture fit', 500, 0.70),
(580, 'Projects & Achievements', 'Notable work, portfolio, and accomplishments', 500, 0.65),
(590, 'Tools & Technology', 'Software, frameworks, languages, and hardware used', 500, 0.65),
(600, 'Home & Lifestyle', 'Where and how the user lives', NULL, 0.60),
(610, 'Current Residence', 'Current address, city, country, and type of home', 600, 0.70),
(620, 'Housing History', 'Previous homes, moves, and living situations', 600, 0.60),
(630, 'Household', 'People living in the same home and household dynamics', 600, 0.65),
(640, 'Interior & Style', 'Aesthetic preferences, décor, and design tastes', 600, 0.55),
(650, 'Possessions', 'Important belongings, collections, and valuables', 600, 0.55),
(660, 'Transport', 'Vehicles owned, commute, and travel within the city', 600, 0.60),
(670, 'Financial', 'Budgeting, income level, savings, and financial goals', 600, 0.75),
(680, 'Shopping', 'Preferred brands, stores, and shopping habits', 600, 0.55),
(690, 'Environmental', 'Sustainability, minimalism, and ecological preferences', 600, 0.55),
(700, 'Entertainment & Leisure', 'How the user spends free time', NULL, 0.60),
(710, 'Music', 'Genres, artists, instruments, and concerts', 700, 0.70),
(720, 'Film & TV', 'Shows, movies, streaming habits, and favourite genres', 700, 0.65),
(730, 'Books & Reading', 'Authors, genres, and reading habits', 700, 0.65),
(740, 'Gaming', 'Video games, board games, and RPG interests', 700, 0.65),
(741, 'Video Games', 'Specific titles, platforms, and gaming preferences', 740, 0.65),
(742, 'Board Games', 'Favourite tabletop games and play style', 740, 0.60),
(750, 'Sports', 'Sports played, watched, and favourite teams', 700, 0.65),
(760, 'Creative Arts', 'Art, craft, photography, writing, and other creative pursuits', 700, 0.60),
(770, 'Collecting & Hobbies', 'Collections and specialised interests', 700, 0.55),
(780, 'Outdoor Activities', 'Hiking, camping, gardening, and nature activities', 700, 0.60),
(790, 'Events & Experiences', 'Concerts, festivals, and memorable experiences', 700, 0.60),
(800, 'Travel & Culture', 'Places visited, desired destinations, and cultural interests', NULL, 0.60),
(810, 'Countries Visited', 'Nations the user has been to or lived in', 800, 0.65),
(820, 'Cities & Landmarks', 'Specific cities, regions, and notable places visited', 800, 0.60),
(830, 'Travel Style', 'Budget, luxury, adventure, and travel preferences', 800, 0.60),
(840, 'Bucket List', 'Desired future destinations and experiences', 800, 0.60),
(850, 'Cultural Interests', 'History, art, traditions, and cultural engagement', 800, 0.55),
(860, 'Travel Languages', 'Languages used or learned for travel', 800, 0.55),
(870, 'Accommodation', 'Hotel, Airbnb, camping, and lodging preferences', 800, 0.55),
(900, 'Schedule & Time', 'Dates, routines, upcoming events, and temporal patterns', NULL, 0.75),
(910, 'Recurring Dates', 'Birthdays, anniversaries, and yearly recurring events', 900, 0.85),
(920, 'Daily Routines', 'Morning, evening, and habitual daily schedules', 900, 0.70),
(930, 'Upcoming Events', 'Appointments, meetings, and scheduled occurrences', 900, 0.80),
(940, 'Past Milestones', 'Significant dates and events that have already passed', 900, 0.65),
(950, 'Seasonal', 'Seasonal preferences and recurring seasonal habits', 900, 0.65),
(960, 'Productivity', 'Time management habits, productivity systems, and scheduling preferences', 900, 0.70);

PRAGMA foreign_keys = ON;
