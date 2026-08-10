//! Entity profile and audit-trail handlers.

use colored::Colorize;
use mimir_api_types::{AuditQueryRequest, ProfileRequest};

use super::{confidence_color, exit_with_error, make_client};

pub async fn handle_kb_profile(entity: Option<String>, json: bool, base_url: &str) {
    let client = make_client(base_url);
    let req = ProfileRequest { entity };
    match client.kb_profile(req).await {
        Ok(resp) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                return;
            }
            println!("Profile for: {}\n", resp.entity_name);
            for group in &resp.groups {
                println!("## {}", group.category);
                for fact in &group.facts {
                    let conf_str = format!("{:.2}", fact.confidence);
                    println!(
                        "  - {} {} {} (confidence: {}, status: {})",
                        fact.subject,
                        fact.predicate,
                        fact.object.clone().unwrap_or_default(),
                        conf_str.color(confidence_color(fact.confidence)),
                        fact.status
                    );
                }
                println!();
            }
        }
        Err(e) => exit_with_error(e),
    }
}

// ------------------------------------------------------------------
// kb audit
// ------------------------------------------------------------------

pub async fn handle_kb_audit(
    entity: Option<String>,
    predicate: Option<String>,
    from: Option<String>,
    to: Option<String>,
    change_type: Option<String>,
    json: bool,
    base_url: &str,
) {
    let client = make_client(base_url);
    let req = AuditQueryRequest {
        entity,
        predicate,
        from,
        to,
        change_type,
        offset: None,
        limit: None,
    };
    match client.kb_audit(req).await {
        Ok(resp) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                return;
            }
            if resp.entries.is_empty() {
                println!("No audit log entries found.");
                return;
            }
            use tabled::{Table, Tabled, settings::Style};
            #[derive(Tabled)]
            struct AuditTableRow {
                audit_id: i32,
                fact_id: i32,
                entity: String,
                predicate: String,
                change_type: String,
                changed_at: String,
            }
            let rows: Vec<AuditTableRow> = resp
                .entries
                .iter()
                .map(|e| AuditTableRow {
                    audit_id: e.audit_id,
                    fact_id: e.fact_id,
                    entity: e.entity_name.clone().unwrap_or_default(),
                    predicate: e.predicate_name.clone().unwrap_or_default(),
                    change_type: e.change_type.clone(),
                    changed_at: e.changed_at.clone(),
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
