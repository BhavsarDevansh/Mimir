-- Add sensitive flag to predicates for bulk-forget safeguard.
ALTER TABLE predicates ADD COLUMN sensitive BOOLEAN NOT NULL DEFAULT FALSE;

-- Seed common sensitive predicates (medical, financial, identity).
UPDATE predicates SET sensitive = TRUE WHERE LOWER(name) IN (
    'allergy',
    'medication',
    'condition',
    'diagnosis',
    'income',
    'salary',
    'password',
    'ssn',
    'social_security_number',
    'bank_account',
    'credit_card',
    'insurance'
);
