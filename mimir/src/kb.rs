use chrono::{DateTime, Utc};
use mimir_core::paths::knowledge_db_path;
use mimir_knowledge::queries::audit::AuditLogFilter;
use mimir_knowledge::{KnowledgeError, KnowledgeGraph};

fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
    // RFC 3339 / ISO 8601 with timezone offset.
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return Some(dt);
    }
    // Naive date → midnight UTC.
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d.and_hms_opt(0, 0, 0)?.and_utc());
    }
    // Naive datetime without timezone.
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

fn parse_change_type(s: &str) -> Option<mimir_knowledge::models::audit_log::ChangeType> {
    match s.to_lowercase().as_str() {
        "created" => Some(mimir_knowledge::models::audit_log::ChangeType::Created),
        "status_change" => Some(mimir_knowledge::models::audit_log::ChangeType::StatusChange),
        "confidence_change" => {
            Some(mimir_knowledge::models::audit_log::ChangeType::ConfidenceChange)
        }
        "temporal_update" => Some(mimir_knowledge::models::audit_log::ChangeType::TemporalUpdate),
        "source_added" => Some(mimir_knowledge::models::audit_log::ChangeType::SourceAdded),
        "forgotten" => Some(mimir_knowledge::models::audit_log::ChangeType::Forgotten),
        "restored" => Some(mimir_knowledge::models::audit_log::ChangeType::Restored),
        _ => None,
    }
}

pub async fn handle_kb_audit(
    entity: Option<String>,
    predicate: Option<String>,
    from: Option<String>,
    to: Option<String>,
    change_type: Option<String>,
) {
    let db_path = match knowledge_db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: could not resolve knowledge DB path: {e}");
            std::process::exit(1);
        }
    };

    let kg = match KnowledgeGraph::init(&db_path).await {
        Ok(g) => g,
        Err(KnowledgeError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("Error: no knowledge graph found. Run `mimir init` first.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: failed to open knowledge graph: {e}");
            std::process::exit(1);
        }
    };

    let ct = change_type.as_deref().and_then(parse_change_type);
    if let Some(ref raw) = change_type {
        if ct.is_none() {
            eprintln!("Error: invalid change_type '{}'", raw);
            std::process::exit(1);
        }
    }

    let from_dt = from.as_deref().and_then(parse_datetime);
    if let Some(ref raw) = from {
        if from_dt.is_none() {
            eprintln!("Error: invalid --from datetime '{}'", raw);
            std::process::exit(1);
        }
    }

    let to_dt = to.as_deref().and_then(parse_datetime);
    if let Some(ref raw) = to {
        if to_dt.is_none() {
            eprintln!("Error: invalid --to datetime '{}'", raw);
            std::process::exit(1);
        }
    }

    let filter = AuditLogFilter {
        entity_name: entity,
        relationship_type_name: predicate,
        from: from_dt,
        to: to_dt,
        change_type: ct,
        limit: None,
        offset: None,
    };

    let rows = match kg.query_audit_log(filter).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error querying audit log: {e}");
            std::process::exit(1);
        }
    };

    if rows.is_empty() {
        println!("No audit log entries found.");
        return;
    }

    println!(
        "{:>6} {:>6} {:<20} {:<15} {:<18} {:<12} {:<25} {:<25}",
        "audit_id",
        "fact_id",
        "entity",
        "predicate",
        "change_type",
        "changed_by",
        "changed_at",
        "reason"
    );
    println!("{}", "-".repeat(130));
    for row in rows {
        let changed_by = row.changed_by_name.as_deref().unwrap_or("-");
        let reason = row.reason.as_deref().unwrap_or("-");
        let entity = row.entity_name.as_deref().unwrap_or("(deleted)");
        let predicate = row.relationship_type_name.as_deref().unwrap_or("(deleted)");
        println!(
            "{:>6} {:>6} {:<20} {:<15} {:<18} {:<12} {:<25} {:<25}",
            row.audit_id,
            row.fact_id,
            entity,
            predicate,
            row.change_type_name,
            changed_by,
            row.changed_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            reason,
        );
    }
}

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

pub async fn handle_kb_forget(input: KbForgetInput) {
    let db_path = match knowledge_db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: could not resolve knowledge DB path: {e}");
            std::process::exit(1);
        }
    };

    let kg = match KnowledgeGraph::init(&db_path).await {
        Ok(g) => g,
        Err(KnowledgeError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("Error: no knowledge graph found. Run `mimir init` first.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: failed to open knowledge graph: {e}");
            std::process::exit(1);
        }
    };

    let filters = mimir_knowledge::forget::ForgetFilters {
        fact_id: input.fact_id,
        predicate: input.predicate,
        subject: input.subject,
        entity: input.entity,
        source: input.source,
        from: input.from.as_deref().and_then(parse_datetime),
        to: input.to.as_deref().and_then(parse_datetime),
        all: input.all,
    };

    if let Some(ref raw) = input.from {
        if filters.from.is_none() {
            eprintln!("Error: invalid --from datetime '{}'", raw);
            std::process::exit(1);
        }
    }
    if let Some(ref raw) = input.to {
        if filters.to.is_none() {
            eprintln!("Error: invalid --to datetime '{}'", raw);
            std::process::exit(1);
        }
    }

    let opts = mimir_knowledge::forget::ForgetOptions {
        yes: input.yes,
        confirm_sensitive: input.confirm_sensitive,
        confirmation_phrase: input.confirmation_phrase,
        archive: input.archive,
    };

    match kg
        .forget_facts(
            filters,
            opts,
            mimir_knowledge::models::audit_log::ChangedBy::User,
        )
        .await
    {
        Ok(result) => {
            if input.all {
                if let Some(path) = result.backup_path {
                    println!("Backup created: {}", path.display());
                }
                println!("{} facts forgotten.", result.forgotten_count);
            } else {
                println!("{} fact(s) moved to trash.", result.forgotten_count);
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

pub async fn handle_kb_restore(trash_id: Option<i32>, all: bool) {
    let db_path = match knowledge_db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: could not resolve knowledge DB path: {e}");
            std::process::exit(1);
        }
    };

    let kg = match KnowledgeGraph::init(&db_path).await {
        Ok(g) => g,
        Err(KnowledgeError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("Error: no knowledge graph found. Run `mimir init` first.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: failed to open knowledge graph: {e}");
            std::process::exit(1);
        }
    };

    if all {
        match kg
            .restore_all(mimir_knowledge::models::audit_log::ChangedBy::User)
            .await
        {
            Ok(facts) => {
                println!("Restored {} fact(s) from trash.", facts.len());
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        let id = match trash_id {
            Some(id) => id,
            None => {
                eprintln!("Error: --trash-id required or use --all");
                std::process::exit(1);
            }
        };
        match kg
            .restore_fact(id, mimir_knowledge::models::audit_log::ChangedBy::User)
            .await
        {
            Ok(fact) => {
                println!("Restored fact {}.", fact.id);
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    }
}

pub async fn handle_kb_trash(empty: bool, limit: u32, offset: u32) {
    let db_path = match knowledge_db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: could not resolve knowledge DB path: {e}");
            std::process::exit(1);
        }
    };

    let kg = match KnowledgeGraph::init(&db_path).await {
        Ok(g) => g,
        Err(KnowledgeError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("Error: no knowledge graph found. Run `mimir init` first.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: failed to open knowledge graph: {e}");
            std::process::exit(1);
        }
    };

    if empty {
        match kg.empty_trash().await {
            Ok(count) => {
                println!("Emptied {} trash row(s).", count);
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        match kg.list_trash(limit as i64, offset as i64).await {
            Ok(items) => {
                if items.is_empty() {
                    println!("Trash is empty.");
                    return;
                }
                println!(
                    "{:<8} {:<20} {:<15} {:<20} {:<25} {:<25}",
                    "trash_id", "subject", "predicate", "object", "deleted_at", "expires_at"
                );
                println!("{}", "-".repeat(113));
                for item in items {
                    let subject = item.subject_name.as_deref().unwrap_or("-");
                    let predicate = item.relationship_type_name.as_deref().unwrap_or("-");
                    let object = item
                        .object_name
                        .as_deref()
                        .or(item.object_literal.as_deref())
                        .unwrap_or("-");
                    println!(
                        "{:<8} {:<20} {:<15} {:<20} {:<25} {:<25}",
                        item.trash_id,
                        subject,
                        predicate,
                        object,
                        item.deleted_at
                            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                        item.expires_at
                            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    );
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    }
}

pub async fn handle_kb_optimization(status: bool, run_now: bool, base_url: &str) {
    let client = mimir_client::MimirClient::new(base_url);

    if status {
        match client.kb_optimization_status().await {
            Ok(resp) => {
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
            Err(e) => {
                eprintln!("Error: failed to fetch optimization status: {e}");
                std::process::exit(1);
            }
        }
    } else if run_now {
        println!("Triggering knowledge graph optimization...");
        match client.kb_optimization_run_now().await {
            Ok(resp) => {
                println!(
                    "Run completed: id={} status={} started_at={} finished_at={:?} error={:?}",
                    resp.run_id, resp.status, resp.started_at, resp.finished_at, resp.error
                );
            }
            Err(e) => {
                eprintln!("Error: failed to run optimization: {e}");
                std::process::exit(1);
            }
        }
    } else {
        eprintln!("Error: specify --status or --run-now");
        std::process::exit(1);
    }
}

pub async fn handle_kb_category(command: crate::cli::CategoryCommands, base_url: &str) {
    let client = mimir_client::MimirClient::new(base_url);
    match command {
        crate::cli::CategoryCommands::List { parent } => {
            let url = match parent {
                Some(id) => format!("{}/kb/categories?parent={}", base_url, id),
                None => format!("{}/kb/categories", base_url),
            };
            match reqwest::get(&url).await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let cats: Vec<serde_json::Value> = resp.json().await.unwrap_or_default();
                        if cats.is_empty() {
                            println!("No categories found.");
                        } else {
                            for cat in cats {
                                let id = cat.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                                let name = cat.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                println!("{:>4} {}", id, name);
                            }
                        }
                    } else {
                        eprintln!("Error: {}", resp.status());
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error: failed to list categories: {}", e);
                    std::process::exit(1);
                }
            }
        }
        crate::cli::CategoryCommands::Show { id } => {
            let url = format!("{}/kb/categories/{}", base_url, id);
            match reqwest::get(&url).await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let cat: serde_json::Value = resp.json().await.unwrap_or_default();
                        println!(
                            "ID:          {}",
                            cat.get("id").and_then(|v| v.as_i64()).unwrap_or(0)
                        );
                        println!(
                            "Name:        {}",
                            cat.get("name").and_then(|v| v.as_str()).unwrap_or("?")
                        );
                        println!(
                            "Description: {}",
                            cat.get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("-")
                        );
                        println!(
                            "Fact count:  {}",
                            cat.get("fact_count").and_then(|v| v.as_i64()).unwrap_or(0)
                        );
                        if let Some(children) = cat.get("children").and_then(|v| v.as_array()) {
                            if !children.is_empty() {
                                println!("Children:");
                                for child in children {
                                    let cid = child.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                                    let cname =
                                        child.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                    println!("  {:>4} {}", cid, cname);
                                }
                            }
                        }
                    } else {
                        eprintln!("Error: {}", resp.status());
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error: failed to show category: {}", e);
                    std::process::exit(1);
                }
            }
        }
        crate::cli::CategoryCommands::Add {
            id,
            name,
            parent,
            description,
        } => {
            let body = serde_json::json!({
                "id": id,
                "name": name,
                "parent_id": parent,
                "description": description,
            });
            let url = format!("{}/kb/categories", base_url);
            match reqwest::Client::new().post(&url).json(&body).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let cat: serde_json::Value = resp.json().await.unwrap_or_default();
                        println!(
                            "Created category {} {}",
                            cat.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                            cat.get("name").and_then(|v| v.as_str()).unwrap_or("?")
                        );
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Error: {}", text);
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error: failed to create category: {}", e);
                    std::process::exit(1);
                }
            }
        }
        crate::cli::CategoryCommands::Delete { id } => {
            let url = format!("{}/kb/categories/{}", base_url, id);
            match reqwest::Client::new().delete(&url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("Deleted category {}", id);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Error: {}", text);
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error: failed to delete category: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}
