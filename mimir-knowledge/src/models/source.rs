//! Source model, source-type enum, and extraction-method enum.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use static_assertions::const_assert;

/// Origin of a fact in the knowledge graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum SourceType {
    UserEdit = 1,
    Connector = 2,
    Inference = 3,
    Interaction = 4,
    Import = 5,
    System = 6,
}

const_assert!((SourceType::UserEdit as i16) != 0);

impl TryFrom<i16> for SourceType {
    type Error = ();

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            x if x == Self::UserEdit as i16 => Ok(Self::UserEdit),
            x if x == Self::Connector as i16 => Ok(Self::Connector),
            x if x == Self::Inference as i16 => Ok(Self::Inference),
            x if x == Self::Interaction as i16 => Ok(Self::Interaction),
            x if x == Self::Import as i16 => Ok(Self::Import),
            x if x == Self::System as i16 => Ok(Self::System),
            _ => Err(()),
        }
    }
}

impl SourceType {
    /// Wire representation of the source type.
    ///
    /// The HTTP API (`mimir-api-types`) carries source types as strings, so
    /// this is the single source of truth for the wire contract — independent
    /// of the derived `Debug` repr (issue #293).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserEdit => "UserEdit",
            Self::Connector => "Connector",
            Self::Inference => "Inference",
            Self::Interaction => "Interaction",
            Self::Import => "Import",
            Self::System => "System",
        }
    }
}

/// How a fact was extracted from its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(i16)]
pub enum ExtractionMethod {
    LlmExtraction = 1,
    StructuredParse = 2,
    UserInput = 3,
    InferenceRule = 4,
    DedupMerge = 5,
}

const_assert!((ExtractionMethod::LlmExtraction as i16) != 0);

/// Provenance record linking a fact to its origin.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct Source {
    pub id: i32,
    pub fact_id: i32,
    pub source_type_id: i16,
    pub connector_instance_id: Option<i32>,
    pub connector_type_id: Option<i16>,
    pub raw_reference: Option<String>,
    pub extracted_at: DateTime<Utc>,
    pub extraction_method_id: Option<i16>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_type_try_from_roundtrip() {
        for id in 1..=6 {
            let ty = SourceType::try_from(id).unwrap();
            assert_eq!(ty as i16, id);
            assert_eq!(SourceType::try_from(ty as i16), Ok(ty));
        }
        assert!(SourceType::try_from(0).is_err());
        assert!(SourceType::try_from(7).is_err());
    }

    #[test]
    fn source_type_as_str_matches_wire_contract() {
        assert_eq!(SourceType::UserEdit.as_str(), "UserEdit");
        assert_eq!(SourceType::Connector.as_str(), "Connector");
        assert_eq!(SourceType::Inference.as_str(), "Inference");
        assert_eq!(SourceType::Interaction.as_str(), "Interaction");
        assert_eq!(SourceType::Import.as_str(), "Import");
        assert_eq!(SourceType::System.as_str(), "System");
    }
}
