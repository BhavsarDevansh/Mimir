//! KB read/edit handlers: query, show, edit, browse.

use colored::Colorize;
use mimir_api_types::{BrowseRequest, FactEditRequest, FactQueryParams};

use super::{confidence_color, exit_with_error, make_client};

pub async fn handle_kb_query(
    entity: String,
    predicate: Option<String>,
    min_confidence: Option<f32>,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
    let req = FactQueryParams {
        entity,
        predicate,
        min_confidence,
        offset: None,
        limit: None,
    };

    match client.kb_query(req).await {
        Ok(resp) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                return;
            }
            if resp.facts.is_empty() {
                println!("No facts found.");
                return;
            }
            use tabled::{Table, Tabled, settings::Style};
            #[derive(Tabled)]
            struct FactTableRow {
                id: i32,
                predicate: String,
                object: String,
                confidence: String,
                status: String,
            }
            let rows: Vec<FactTableRow> = resp
                .facts
                .iter()
                .map(|f| FactTableRow {
                    id: f.id,
                    predicate: f.predicate.clone(),
                    object: f.object.clone().unwrap_or_default(),
                    confidence: format!("{:.2}", f.confidence),
                    status: f.status.clone(),
                })
                .collect();
            let mut table = Table::new(rows);
            table.with(Style::modern());
            println!("{}", table);
            println!("Total: {}", resp.total);
        }
        Err(e) => exit_with_error(e),
    }
}

// ------------------------------------------------------------------
// kb show
// ------------------------------------------------------------------

pub async fn handle_kb_show(
    fact_id: i32,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
    match client.kb_show(fact_id).await {
        Ok(resp) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                return;
            }
            let f = &resp.fact;
            println!("Fact ID:       {}", f.id);
            println!("Subject:       {}", f.subject);
            println!("Predicate:     {}", f.predicate);
            println!("Object:        {}", f.object.clone().unwrap_or_default());
            let conf_str = format!("{:.2}", f.confidence);
            println!(
                "Confidence:    {}",
                conf_str.color(confidence_color(f.confidence))
            );
            println!("Status:        {}", f.status);
            println!("Inferred:      {}", f.inferred);
            if let Some(ref vf) = f.valid_from {
                println!("Valid from:    {}", vf);
            }
            if let Some(ref vu) = f.valid_until {
                println!("Valid until:   {}", vu);
            }
            if !resp.sources.is_empty() {
                println!("\nSources:");
                for s in &resp.sources {
                    println!("  - {} ({})", s.source_type, s.extracted_at);
                    if let Some(ref rid) = s.raw_reference {
                        println!("    Reference: {}", rid);
                    }
                }
            }
            if !resp.dependencies.is_empty() {
                println!("\nDependencies:");
                for d in &resp.dependencies {
                    println!(
                        "  - {}: {} -> {}",
                        d.relation_type, d.parent_fact_id, d.child_fact_id
                    );
                }
            }
            if !resp.audit_log.is_empty() {
                println!("\nAudit log (last 10):");
                for a in resp.audit_log.iter().take(10) {
                    println!(
                        "  [{}] {}: {} -> {} by {} at {}",
                        a.audit_id,
                        a.change_type,
                        a.old_value.as_deref().unwrap_or("-"),
                        a.new_value.as_deref().unwrap_or("-"),
                        a.changed_by.as_deref().unwrap_or("?"),
                        a.changed_at
                    );
                }
            }
        }
        Err(e) => exit_with_error(e),
    }
}

// ------------------------------------------------------------------
// kb edit
// ------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn handle_kb_edit(
    fact_id: i32,
    confidence: Option<f32>,
    valid_from: Option<String>,
    valid_until: Option<String>,
    object: Option<String>,
    status: Option<String>,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
    let req = FactEditRequest {
        confidence,
        valid_from,
        valid_until,
        object_literal: object,
        status,
    };
    match client.kb_edit(fact_id, req).await {
        Ok(resp) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                return;
            }
            let f = &resp.fact;
            println!("Updated fact {}:", f.id);
            println!(
                "  {} {} {} (confidence: {:.2}, status: {})",
                f.subject,
                f.predicate,
                f.object.clone().unwrap_or_default(),
                f.confidence,
                f.status
            );
        }
        Err(e) => exit_with_error(e),
    }
}

// ------------------------------------------------------------------
// kb browse
// ------------------------------------------------------------------

pub async fn handle_kb_browse(
    entity: String,
    depth: Option<u32>,
    limit: u32,
    offset: u32,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
    let req = BrowseRequest {
        entity,
        depth: depth.unwrap_or(2).min(5),
        offset: Some(offset),
        limit: Some(limit),
    };
    match client.kb_browse(req).await {
        Ok(resp) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                return;
            }
            if resp.edges.is_empty() {
                println!("No connections found.");
                return;
            }
            println!(
                "Browsed {} edges (total {}):",
                resp.edges.len(),
                resp.total_edges
            );
            for edge in &resp.edges {
                let indent = "  ".repeat(edge.depth as usize);
                let conf_str = format!("{:.2}", edge.confidence);
                println!(
                    "{}{} --[{}]--> {} ({})",
                    indent,
                    edge.subject,
                    edge.predicate,
                    edge.object,
                    conf_str.color(confidence_color(edge.confidence))
                );
            }
        }
        Err(e) => exit_with_error(e),
    }
}
