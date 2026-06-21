use chrono::{DateTime, Utc};
use colored::Colorize;
use mimir_api_types::{
    AuditQueryRequest, BrowseRequest, FactEditRequest, FactQueryParams, ForgetRequest,
    ProfileRequest, RestoreRequest,
};
use mimir_client::MimirClient;

#[allow(dead_code)]
fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return Some(dt);
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0).map(|t| t.and_utc());
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
    ] {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(ndt.and_utc());
        }
    }
    None
}

// ------------------------------------------------------------------
// Shared error helper
// ------------------------------------------------------------------

fn exit_with_error(msg: impl std::fmt::Display) -> ! {
    eprintln!("Error: {}", msg);
    std::process::exit(1);
}

fn make_client(base_url: &str) -> MimirClient {
    MimirClient::new(base_url)
}

// ------------------------------------------------------------------
// Confidence color helper
// ------------------------------------------------------------------

fn confidence_color(conf: f32) -> colored::Color {
    if conf > 0.9 {
        colored::Color::Green
    } else if conf >= 0.7 {
        colored::Color::Yellow
    } else {
        colored::Color::Red
    }
}

// ------------------------------------------------------------------
// kb query
// ------------------------------------------------------------------

pub async fn handle_kb_query(
    entity: String,
    predicate: Option<String>,
    min_confidence: Option<f32>,
    json: bool,
    base_url: &str,
) {
    let client = make_client(base_url);
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

pub async fn handle_kb_show(fact_id: i32, json: bool, base_url: &str) {
    let client = make_client(base_url);
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
    base_url: &str,
) {
    let client = make_client(base_url);
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
    base_url: &str,
) {
    let client = make_client(base_url);
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

// ------------------------------------------------------------------
// kb profile
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// kb forget
// ------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct KbForgetInput {
    pub fact_id: Option<i32>,
    pub predicate: Option<String>,
    pub subject: Option<String>,
    pub entity: Option<String>,
    pub source: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub all: bool,
    pub yes: bool,
    pub confirm_sensitive: bool,
    pub archive: bool,
    pub confirmation_phrase: Option<String>,
}

pub async fn handle_kb_forget(input: KbForgetInput, base_url: &str) {
    let client = make_client(base_url);
    let req = ForgetRequest {
        fact_id: input.fact_id,
        predicate: input.predicate,
        subject: input.subject,
        entity: input.entity,
        source: input.source,
        from: input.from,
        to: input.to,
        all: input.all,
        yes: input.yes,
        confirm_sensitive: input.confirm_sensitive,
        confirmation_phrase: input.confirmation_phrase,
        archive: input.archive,
    };
    match client.kb_forget(req).await {
        Ok(resp) => {
            if input.all {
                if let Some(ref path) = resp.backup_path {
                    println!("Backup created: {}", path);
                }
                println!("{} facts forgotten.", resp.forgotten_count);
            } else {
                println!("{} fact(s) moved to trash.", resp.forgotten_count);
            }
        }
        Err(e) => exit_with_error(e),
    }
}

// ------------------------------------------------------------------
// kb restore
// ------------------------------------------------------------------

pub async fn handle_kb_restore(trash_id: Option<i32>, all: bool, base_url: &str) {
    let client = make_client(base_url);
    let req = RestoreRequest { trash_id, all };
    match client.kb_restore(req).await {
        Ok(resp) => {
            println!("Restored {} fact(s) from trash.", resp.restored_count);
        }
        Err(e) => exit_with_error(e),
    }
}

// ------------------------------------------------------------------
// kb trash
// ------------------------------------------------------------------

pub async fn handle_kb_trash(empty: bool, limit: u32, offset: u32, json: bool, base_url: &str) {
    let client = make_client(base_url);
    if empty {
        match client.kb_trash_empty().await {
            Ok(()) => {
                println!("Trash emptied.");
            }
            Err(e) => exit_with_error(e),
        }
        return;
    }
    match client.kb_trash(offset, limit).await {
        Ok(resp) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                return;
            }
            if resp.items.is_empty() {
                println!("Trash is empty.");
                return;
            }
            use tabled::{Table, Tabled, settings::Style};
            #[derive(Tabled)]
            struct TrashTableRow {
                trash_id: i32,
                subject: String,
                predicate: String,
                object: String,
                deleted_at: String,
                expires_at: String,
            }
            let rows: Vec<TrashTableRow> = resp
                .items
                .iter()
                .map(|i| TrashTableRow {
                    trash_id: i.trash_id,
                    subject: i.subject.clone().unwrap_or_default(),
                    predicate: i.predicate.clone().unwrap_or_default(),
                    object: i.object.clone().unwrap_or_default(),
                    deleted_at: i.deleted_at.clone(),
                    expires_at: i.expires_at.clone(),
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
// kb optimization
// ------------------------------------------------------------------

pub async fn handle_kb_optimization(status: bool, run_now: bool, json: bool, base_url: &str) {
    let client = make_client(base_url);
    if status {
        match client.kb_optimization_status().await {
            Ok(resp) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                    return;
                }
                println!("Job ID: {}", resp.job_id);
                println!("Priority: {}", resp.priority);
                if let Some(schedule) = resp.schedule {
                    println!("Schedule: {}", schedule);
                }
                if let Some(next) = resp.next_run_at {
                    println!("Next run: {}", next);
                }
                if let Some(last) = resp.last_run {
                    println!(
                        "Last run: id={} status={} started_at={} finished_at={:?} error={:?}",
                        last.run_id, last.status, last.started_at, last.finished_at, last.error
                    );
                } else {
                    println!("Last run: never");
                }
            }
            Err(e) => exit_with_error(format!("failed to fetch optimization status: {e}")),
        }
    } else if run_now {
        match client.kb_optimization_run_now().await {
            Ok(resp) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                    return;
                }
                println!(
                    "Optimization run id={} status={} started_at={} finished_at={:?} error={:?}",
                    resp.run_id, resp.status, resp.started_at, resp.finished_at, resp.error
                );
            }
            Err(e) => exit_with_error(format!("failed to run optimization: {e}")),
        }
    }
}

// ------------------------------------------------------------------
// kb category
// ------------------------------------------------------------------

pub async fn handle_kb_category(command: crate::cli::CategoryCommands, base_url: &str) {
    match command {
        crate::cli::CategoryCommands::List { parent } => {
            let client = make_client(base_url);
            match client.kb_categories(parent).await {
                Ok(cats) => {
                    if cats.is_empty() {
                        println!("No categories.");
                        return;
                    }
                    use tabled::{Table, Tabled, settings::Style};
                    #[derive(Tabled)]
                    struct CatRow {
                        id: i32,
                        name: String,
                        parent_id: String,
                        description: String,
                    }
                    let rows: Vec<CatRow> = cats
                        .into_iter()
                        .map(|c| CatRow {
                            id: c.id,
                            name: c.name,
                            parent_id: c
                                .parent_id
                                .map(|i| i.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                            description: c.description.unwrap_or_else(|| "-".to_string()),
                        })
                        .collect();
                    let mut table = Table::new(rows);
                    table.with(Style::modern());
                    println!("{}", table);
                }
                Err(e) => exit_with_error(format!("failed to list categories: {e}")),
            }
        }
        crate::cli::CategoryCommands::Show { id } => {
            let client = make_client(base_url);
            match client.kb_category_show(id).await {
                Ok(cat) => {
                    println!("ID:          {}", cat.id);
                    println!("Name:        {}", cat.name);
                    println!(
                        "Description: {}",
                        cat.description.unwrap_or_else(|| "-".to_string())
                    );
                    println!("Fact count:  {}", cat.fact_count);
                    if !cat.children.is_empty() {
                        println!("Children:");
                        for child in cat.children {
                            println!("  {:>4} {}", child.id, child.name);
                        }
                    }
                }
                Err(e) => exit_with_error(format!("failed to show category: {e}")),
            }
        }
        crate::cli::CategoryCommands::Add {
            id,
            name,
            parent,
            description,
            memory_weight,
        } => {
            let client = make_client(base_url);
            match client
                .kb_category_create(id, name, parent, description, memory_weight)
                .await
            {
                Ok(cat) => {
                    println!("Created category {} {}", cat.id, cat.name);
                }
                Err(e) => exit_with_error(format!("failed to create category: {e}")),
            }
        }
        crate::cli::CategoryCommands::Delete { id } => {
            let client = make_client(base_url);
            match client.kb_category_delete(id).await {
                Ok(()) => println!("Deleted category {}", id),
                Err(e) => exit_with_error(format!("failed to delete category: {e}")),
            }
        }
    }
}

// ------------------------------------------------------------------
// kb pending / confirm / reject (issue #141)
// ------------------------------------------------------------------

/// List sensitive facts awaiting confirmation.
pub async fn handle_kb_pending(json: bool, base_url: &str) {
    let client = make_client(base_url);
    match client.kb_pending().await {
        Ok(resp) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                return;
            }
            if resp.facts.is_empty() {
                println!("No pending sensitive facts.");
                return;
            }
            println!("Pending sensitive facts ({}):", resp.total);
            println!(
                "{:<8} {:<20} {:<20} {:<24} Created",
                "ID", "Subject", "Predicate", "Object"
            );
            for f in &resp.facts {
                println!(
                    "{:<8} {:<20} {:<20} {:<24} {}",
                    f.fact_id,
                    truncate(&f.subject, 20),
                    truncate(&f.predicate, 20),
                    truncate(f.object.as_deref().unwrap_or("-"), 24),
                    f.created_at
                );
            }
        }
        Err(e) => exit_with_error(e),
    }
}

/// Confirm a pending sensitive fact.
pub async fn handle_kb_confirm(fact_id: i32, json: bool, base_url: &str) {
    let client = make_client(base_url);
    match client.kb_confirm(fact_id).await {
        Ok(resp) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                return;
            }
            let f = &resp.fact;
            println!("Confirmed fact {}:", f.id);
            println!(
                "  {} {} {}",
                f.subject,
                f.predicate,
                f.object.clone().unwrap_or_default()
            );
            let conf_str = format!("{:.2}", f.confidence);
            println!(
                "  Confidence: {}",
                conf_str.color(confidence_color(f.confidence))
            );
            println!("  Status:     {}", f.status);
        }
        Err(e) => exit_with_error(e),
    }
}

/// Reject a pending sensitive fact.
pub async fn handle_kb_reject(fact_id: i32, reason: Option<String>, base_url: &str) {
    let client = make_client(base_url);
    match client.kb_reject(fact_id, reason.as_deref()).await {
        Ok(()) => println!("Rejected and deleted fact {}.", fact_id),
        Err(e) => exit_with_error(e),
    }
}

/// Truncate a string to `max` chars, appending an ellipsis if truncated.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{truncated}…")
}
