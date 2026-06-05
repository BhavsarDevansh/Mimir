#![allow(dead_code)]
//! Shared test helpers for mimir-knowledge integration tests.

use chrono::{DateTime, Utc};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::fact::{Fact, NewFact};
use mimir_knowledge::models::source::SourceType;

pub struct TestGraph {
    pub kg: KnowledgeGraph,
    pub _dir: tempfile::TempDir,
}

impl TestGraph {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
            .await
            .unwrap();
        Self { kg, _dir: dir }
    }

    pub async fn create_person(&self, name: &str) -> i32 {
        let entity = self
            .kg
            .create_entity(name, EntityType::Person, &[])
            .await
            .unwrap();
        entity.id
    }

    pub async fn create_place(&self, name: &str) -> i32 {
        let entity = self
            .kg
            .create_entity(name, EntityType::Place, &[])
            .await
            .unwrap();
        entity.id
    }

    pub async fn create_activity(&self, name: &str) -> i32 {
        let entity = self
            .kg
            .create_entity(name, EntityType::Activity, &[])
            .await
            .unwrap();
        entity.id
    }

    pub async fn create_fact(
        &self,
        subject: i32,
        predicate_name: &str,
        object: Option<i32>,
        source_type: SourceType,
    ) -> Fact {
        let new_fact = NewFact {
            subject_id: subject,
            relationship_type: predicate_name.to_string(),
            object_id: object,
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        };
        self.kg.insert_fact(new_fact).await.unwrap()
    }

    pub async fn create_fact_with_temporal(
        &self,
        subject: i32,
        predicate_name: &str,
        object: Option<i32>,
        valid_from: Option<DateTime<Utc>>,
        valid_until: Option<DateTime<Utc>>,
        source_type: SourceType,
    ) -> Fact {
        let new_fact = NewFact {
            subject_id: subject,
            relationship_type: predicate_name.to_string(),
            object_id: object,
            object_literal: None,
            valid_from,
            valid_until,
            source_type,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        };
        self.kg.insert_fact(new_fact).await.unwrap()
    }
}
