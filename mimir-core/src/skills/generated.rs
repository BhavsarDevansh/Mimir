/// Summary of a completed session used to evaluate skill generation triggers.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// Number of distinct tools invoked during the session.
    pub tool_count: usize,
    /// Whether the task succeeded (user did not correct / reject output).
    pub success: bool,
    /// Whether a similar task pattern has been seen in recent sessions.
    pub novel_pattern: bool,
    /// Agent's confidence in the synthesized answer (0.0–1.0).
    pub confidence: f64,
}

/// A candidate system-generated skill.
///
/// TODO(#20): This type is currently unused. It will be populated by the
/// post-session reflection loop in Phase B.
#[derive(Debug, Clone)]
pub struct GeneratedSkillCandidate {
    pub name: String,
    pub session_summary: SessionSummary,
    pub task_summary: String,
    pub tools_used: Vec<String>,
}

/// Evaluate whether the conditions for auto-generating a skill are met.
///
/// All four conditions must be true:
/// 1. Task required ≥ 3 distinct tools
/// 2. Task succeeded
/// 3. Similar task pattern has not been seen recently
/// 4. Agent's confidence > 0.85
pub fn should_generate_skill(summary: &SessionSummary) -> bool {
    summary.tool_count >= 3 && summary.success && summary.novel_pattern && summary.confidence > 0.85
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_generate_when_all_conditions_met() {
        let summary = SessionSummary {
            tool_count: 3,
            success: true,
            novel_pattern: true,
            confidence: 0.9,
        };
        assert!(should_generate_skill(&summary));
    }

    #[test]
    fn should_not_generate_when_tool_count_too_low() {
        let summary = SessionSummary {
            tool_count: 2,
            success: true,
            novel_pattern: true,
            confidence: 0.9,
        };
        assert!(!should_generate_skill(&summary));
    }

    #[test]
    fn should_not_generate_when_failed() {
        let summary = SessionSummary {
            tool_count: 3,
            success: false,
            novel_pattern: true,
            confidence: 0.9,
        };
        assert!(!should_generate_skill(&summary));
    }

    #[test]
    fn should_not_generate_when_not_novel() {
        let summary = SessionSummary {
            tool_count: 3,
            success: true,
            novel_pattern: false,
            confidence: 0.9,
        };
        assert!(!should_generate_skill(&summary));
    }

    #[test]
    fn should_not_generate_when_confidence_too_low() {
        let summary = SessionSummary {
            tool_count: 3,
            success: true,
            novel_pattern: true,
            confidence: 0.8,
        };
        assert!(!should_generate_skill(&summary));
    }
}
