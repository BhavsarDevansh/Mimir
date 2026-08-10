//! Supervisor error types.
//!
//! [`SupervisorError`] covers the control-plane failures surfaced by
//! [`super::ConnectorSupervisor`] (knowledge-graph, connector, and JSON
//! config errors). [`ActError`] is the narrower dispatch error set raised
//! by [`ConnectorSupervisor::act`](super::runner::ConnectorSupervisor::act), with a lossless conversion from
//! [`SupervisorError`].

use crate::ConnectorError;

/// Errors raised by supervisor operations.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error(transparent)]
    Knowledge(#[from] mimir_knowledge::KnowledgeError),
    #[error(transparent)]
    Connector(#[from] crate::ConnectorError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The connector row exists but its `connector_type_id` does not map to a
    /// known [`ConnectorType`](mimir_knowledge::models::enums::ConnectorType) (Phase 3 A2 / #203).
    #[error("connector {id} has an unknown connector_type id {type_id}")]
    UnknownConnectorType { id: i32, type_id: i16 },
}

/// Errors raised by [`ConnectorSupervisor::act`](super::runner::ConnectorSupervisor::act) (Phase 3 A2 / #203).
///
/// Infrastructure failures of the dispatch mechanism: an unknown instance, an
/// unresolvable connector type, a knowledge-graph lookup failure, or the
/// connector's own [`ConnectorError`] (e.g. `UnsupportedAction`).
#[derive(Debug, thiserror::Error)]
pub enum ActError {
    #[error(transparent)]
    Knowledge(#[from] mimir_knowledge::KnowledgeError),
    /// No connector row exists with the given instance id.
    #[error("no connector with id {0}")]
    NotFound(i32),
    /// The connector row exists but its `connector_type_id` does not map to a
    /// known [`ConnectorType`](mimir_knowledge::models::enums::ConnectorType).
    #[error("connector {id} has an unknown connector_type id {type_id}")]
    UnknownType { id: i32, type_id: i16 },
    #[error(transparent)]
    Connector(#[from] crate::ConnectorError),
}

impl From<SupervisorError> for ActError {
    fn from(error: SupervisorError) -> Self {
        match error {
            SupervisorError::Knowledge(ke) => ActError::Knowledge(ke),
            SupervisorError::Connector(ce) => ActError::Connector(ce),
            // A malformed `config_json` is a connector configuration fault.
            SupervisorError::Json(je) => {
                ActError::Connector(ConnectorError::Config(je.to_string()))
            }
            SupervisorError::UnknownConnectorType { id, type_id } => {
                ActError::UnknownType { id, type_id }
            }
        }
    }
}
