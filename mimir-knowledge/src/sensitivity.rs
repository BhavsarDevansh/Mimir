//! Deterministic sensitivity detection for extracted facts.
//!
//! Sensitivity is validated in Rust, not delegated to the LLM. The LLM still
//! provides an initial `is_sensitive` flag via the `remember` tool, but Rust
//! has the final say using two independent signals:
//!
//! 1. **Category check** — does the fact belong to a known sensitive catalogue
//!    category? The category IDs are assigned by the LLM during extraction and
//!    validated against the Dewey-Decimal taxonomy.
//! 2. **Content check** — does the fact's object text contain a sensitive
//!    keyword? This is the safety net for when the LLM miscategorises or omits
//!    categories entirely.
//!
//! ## Decision logic (AND gate)
//!
//! The LLM flag is a **gate** — Rust can only *narrow*, never widen:
//!
//! | LLM says | Rust says | Result |
//! |----------|-----------|--------|
//! | sensitive | sensitive | **sensitive** |
//! | sensitive | non-sensitive | **non-sensitive** (Rust overrides) |
//! | non-sensitive | anything | **non-sensitive** |
//!
//! This eliminates the false-positive problem (#142) where benign preferences
//! like "I don't like chihuahuas" were routed into pending confirmation.

/// Catalogue category IDs whose facts require user confirmation before being
/// stored permanently.
///
/// Derived from the Dewey-Decimal taxonomy seeded in migration `031`. Only
/// categories that map to the VISION doc's sensitivity definition (health,
/// financial, relationship status, religious/political beliefs, legal status)
/// are listed here. Lifestyle sub-categories of Health & Wellness that are not
/// inherently sensitive (fitness routines, sleep schedules, general nutrition)
/// are deliberately excluded so that Rust can override LLM false positives.
///
/// To amend the set, simply add or remove a category ID here with a comment
/// explaining why.
pub const SENSITIVE_CATEGORIES: &[i32] = &[
    150, // Cultural & Religious — faith, traditions, holidays observed
    180, // Values & Philosophy — political stance, ethical principles, worldview
    230, // Allergies & Intolerances — medical food reactions
    300, // Health & Wellness — root; catches facts tagged at the top level
    310, // Medical History — past diagnoses, surgeries
    320, // Current Conditions — ongoing health conditions
    330, // Medications & Treatments — prescriptions, therapies
    340, // Healthcare Providers — doctors, hospitals, insurance
    370, // Mental Health — therapy, diagnoses, emotional wellbeing
    390, // Disabilities & Accessibility — physical or cognitive needs
    420, // Romantic — partner, spouse, dating history, relationship status
    670, // Financial — budgeting, income level, savings, financial goals
];

/// Keywords that indicate a sensitive fact when found (case-insensitively) as
/// a whole word in the fact's object text.
///
/// This is the fallback for cases where the LLM uses a non-standard category or
/// assigns no category at all. The list is intentionally focused on terms that
/// unambiguously signal health, financial, relationship, or legal sensitivity.
pub const SENSITIVE_KEYWORDS: &[&str] = &[
    // Health
    "allergic",
    "allergy",
    "diabetic",
    "diabetes",
    "medication",
    "prescription",
    "diagnosis",
    "diagnosed",
    "surgery",
    "surgical",
    "hospital",
    "therapy",
    "therapist",
    "psychiatrist",
    "depression",
    "anxiety",
    // Financial
    "debt",
    "bankrupt",
    "bankruptcy",
    "salary",
    "mortgage",
    "loan",
    "investment",
    // Relationship / legal
    "divorce",
    "separated",
    "citizenship",
    "passport",
    "visa",
];

/// Returns `true` if any of the given category IDs is in the
/// [`SENSITIVE_CATEGORIES`] set.
///
/// Pure and synchronous — no database access. The caller is responsible for
/// passing validated category IDs (already checked against the taxonomy).
pub fn is_sensitive_by_category(category_ids: &[i32]) -> bool {
    category_ids
        .iter()
        .any(|id| SENSITIVE_CATEGORIES.contains(id))
}

/// Returns `true` if the object text contains any [`SENSITIVE_KEYWORDS`] entry
/// as a case-insensitive whole word (using word boundaries).
///
/// Pure and synchronous. This is the fallback when category-based detection
/// misses (e.g. the LLM assigned a non-sensitive or no category).
///
/// Matching uses word boundaries rather than raw substrings so that a keyword
/// embedded in a benign word does not trigger a false positive (e.g.
/// `"hospital"` inside `"hospitality"`, `"debt"` inside `"indebted"`, or
/// `"visa"` inside `"visage"`).
pub fn is_sensitive_by_content(object: &str) -> bool {
    let lower = object.to_lowercase();
    SENSITIVE_KEYWORDS
        .iter()
        .any(|kw| contains_keyword_word(&lower, kw))
}

/// Checks whether `keyword` appears in `text` as a whole word (using ASCII
/// alphanumeric boundaries), not as a substring inside a larger word.
///
/// This avoids false positives where a sensitive keyword is embedded in a
/// benign word (e.g. `"hospital"` inside `"hospitality"`, `"debt"` inside
/// `"indebted"`, `"visa"` inside `"visage"`). Keyword entries are all-ASCII, so
/// a byte-level boundary check on ASCII alphanumeric characters is sufficient;
/// any non-ASCII character is treated as a word boundary.
fn contains_keyword_word(text: &str, keyword: &str) -> bool {
    let text_bytes = text.as_bytes();
    let kw_bytes = keyword.as_bytes();
    let kw_len = kw_bytes.len();
    if kw_len == 0 || kw_len > text.len() {
        return false;
    }

    let is_word_byte = |b: u8| b.is_ascii_alphanumeric();
    let mut search_from = 0;
    while let Some(offset) = text[search_from..].find(keyword) {
        let start = search_from + offset;
        let end = start + kw_len;

        let before_is_boundary = start == 0 || !is_word_byte(text_bytes[start - 1]);
        let after_is_boundary = end == text.len() || !is_word_byte(text_bytes[end]);

        if before_is_boundary && after_is_boundary {
            return true;
        }

        search_from = start + 1;
    }

    false
}

/// Combined sensitivity gate implementing the AND logic from issue #142.
///
/// A fact is sensitive **only if** the LLM flagged it **and** Rust's category or
/// content check agrees. Rust can narrow the LLM's assessment but never widen
/// it.
///
/// # Examples
///
/// ```
/// # use mimir_knowledge::sensitivity::is_sensitive;
/// // LLM says sensitive + Rust says sensitive → sensitive
/// assert!(is_sensitive(true, &[320], "diabetes"));
/// // LLM says sensitive + Rust says non-sensitive → non-sensitive
/// assert!(!is_sensitive(true, &[610], "small flat"));
/// // LLM says non-sensitive → non-sensitive regardless of Rust
/// assert!(!is_sensitive(false, &[320], "diabetes"));
/// ```
pub fn is_sensitive(llm_flag: bool, category_ids: &[i32], object: &str) -> bool {
    llm_flag && (is_sensitive_by_category(category_ids) || is_sensitive_by_content(object))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // is_sensitive_by_category
    // -----------------------------------------------------------------------

    #[test]
    fn category_sensitive_root_health() {
        assert!(is_sensitive_by_category(&[300]));
    }

    #[test]
    fn category_sensitive_child_medical_history() {
        assert!(is_sensitive_by_category(&[310]));
    }

    #[test]
    fn category_sensitive_allergies() {
        assert!(is_sensitive_by_category(&[230]));
    }

    #[test]
    fn category_sensitive_financial() {
        assert!(is_sensitive_by_category(&[670]));
    }

    #[test]
    fn category_sensitive_romantic() {
        assert!(is_sensitive_by_category(&[420]));
    }

    #[test]
    fn category_sensitive_religious() {
        assert!(is_sensitive_by_category(&[150]));
    }

    #[test]
    fn category_sensitive_political() {
        assert!(is_sensitive_by_category(&[180]));
    }

    #[test]
    fn category_non_sensitive_residence() {
        assert!(!is_sensitive_by_category(&[610]));
    }

    #[test]
    fn category_non_sensitive_hobby() {
        assert!(!is_sensitive_by_category(&[770]));
    }

    #[test]
    fn category_non_sensitive_food_tastes() {
        assert!(!is_sensitive_by_category(&[210]));
    }

    #[test]
    fn category_non_sensitive_family() {
        assert!(!is_sensitive_by_category(&[410]));
    }

    #[test]
    fn category_non_sensitive_fitness() {
        // Fitness is under 300 but deliberately excluded — workout routines
        // are lifestyle, not sensitive health data.
        assert!(!is_sensitive_by_category(&[350]));
    }

    #[test]
    fn category_mixed_ids_one_sensitive() {
        assert!(is_sensitive_by_category(&[610, 320, 770]));
    }

    #[test]
    fn category_all_non_sensitive() {
        assert!(!is_sensitive_by_category(&[610, 770, 210]));
    }

    #[test]
    fn category_empty_list() {
        assert!(!is_sensitive_by_category(&[]));
    }

    // -----------------------------------------------------------------------
    // is_sensitive_by_content
    // -----------------------------------------------------------------------

    #[test]
    fn content_allergic() {
        assert!(is_sensitive_by_content("allergic to peanuts"));
    }

    #[test]
    fn content_diabetes() {
        assert!(is_sensitive_by_content("diabetes"));
    }

    #[test]
    fn content_medication() {
        assert!(is_sensitive_by_content(
            "taking medication for blood pressure"
        ));
    }

    #[test]
    fn content_salary() {
        assert!(is_sensitive_by_content("my salary is 100k"));
    }

    #[test]
    fn content_debt() {
        assert!(is_sensitive_by_content("I have debt"));
    }

    #[test]
    fn content_citizenship() {
        assert!(is_sensitive_by_content("applying for citizenship"));
    }

    #[test]
    fn content_divorce() {
        assert!(is_sensitive_by_content("going through a divorce"));
    }

    #[test]
    fn content_case_insensitive() {
        assert!(is_sensitive_by_content("ALLERGIC"));
        assert!(is_sensitive_by_content("DiAbEtEs"));
    }

    #[test]
    fn content_non_sensitive_small_flat() {
        assert!(!is_sensitive_by_content("small flat"));
    }

    #[test]
    fn content_non_sensitive_coding() {
        assert!(!is_sensitive_by_content("coding"));
    }

    #[test]
    fn content_non_sensitive_chihuahuas() {
        assert!(!is_sensitive_by_content("chihuahuas"));
    }

    #[test]
    fn content_empty() {
        assert!(!is_sensitive_by_content(""));
    }

    // Word-boundary false positives from PR review (issue #142).

    #[test]
    fn content_hospitality_not_hospital() {
        assert!(!is_sensitive_by_content("I work in hospitality"));
    }

    #[test]
    fn content_indebted_not_debt() {
        assert!(!is_sensitive_by_content("I feel indebted to my teacher"));
    }

    #[test]
    fn content_visage_not_visa() {
        assert!(!is_sensitive_by_content("her visage"));
    }

    #[test]
    fn content_keyword_with_trailing_punctuation() {
        assert!(is_sensitive_by_content("diabetes."));
        assert!(is_sensitive_by_content("allergic, peanuts"));
    }

    #[test]
    fn content_genuine_hospital_word() {
        assert!(is_sensitive_by_content("admitted to hospital"));
    }

    // -----------------------------------------------------------------------
    // is_sensitive (combined AND gate)
    // -----------------------------------------------------------------------

    // Acceptance criteria test cases from issue #142.

    #[test]
    fn combined_allergic_to_peanuts_is_sensitive() {
        // Category 230 (Allergies) or keyword "allergic" — both catch it.
        assert!(is_sensitive(true, &[230], "peanuts"));
        assert!(is_sensitive(true, &[], "allergic to peanuts"));
    }

    #[test]
    fn combined_live_in_small_flat_is_non_sensitive() {
        assert!(!is_sensitive(true, &[610], "small flat"));
    }

    #[test]
    fn combined_salary_100k_is_sensitive() {
        assert!(is_sensitive(true, &[670], "$100k"));
        assert!(is_sensitive(true, &[], "my salary is $100k"));
    }

    #[test]
    fn combined_like_coding_is_non_sensitive() {
        assert!(!is_sensitive(true, &[540], "coding"));
    }

    #[test]
    fn combined_have_diabetes_is_sensitive() {
        assert!(is_sensitive(true, &[320], "diabetes"));
    }

    #[test]
    fn combined_dont_like_chihuahuas_is_non_sensitive() {
        assert!(!is_sensitive(true, &[220], "chihuahuas"));
    }

    // LLM gate tests — Rust cannot widen.

    #[test]
    fn combined_llm_false_always_non_sensitive() {
        assert!(!is_sensitive(false, &[320], "diabetes"));
        assert!(!is_sensitive(false, &[670], "salary"));
    }

    #[test]
    fn combined_llm_true_rust_false_overrides_to_non_sensitive() {
        assert!(!is_sensitive(true, &[610], "small flat"));
        assert!(!is_sensitive(true, &[770], "coding"));
    }
}
