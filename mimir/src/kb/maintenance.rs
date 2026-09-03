//! KB maintenance handlers: forget, restore, trash, optimization, category,
//! and pending-fact confirmation flows.

use colored::Colorize;
use is_terminal::IsTerminal;
use mimir_api_types::{ForgetRequest, RestoreRequest};
use mimir_client::MimirClient;
use std::time::Duration;

use super::{confidence_color, exit_with_error, make_client, truncate};
use crate::connector::wizard::{InquirePrompt, PromptDriver};

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

pub async fn handle_kb_forget(input: KbForgetInput, transport: &crate::transport::DaemonTransport) {
    let client = make_client(transport);
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

pub async fn handle_kb_restore(
    trash_id: Option<i32>,
    all: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
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

pub async fn handle_kb_trash(
    empty: bool,
    limit: u32,
    offset: u32,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
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

pub async fn handle_kb_optimization(
    status: bool,
    run_now: bool,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
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

pub async fn handle_kb_category(
    command: crate::cli::CategoryCommands,
    transport: &crate::transport::DaemonTransport,
) {
    match command {
        crate::cli::CategoryCommands::List { parent } => {
            let client = make_client(transport);
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
            let client = make_client(transport);
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
            memory_bucket_id,
        } => {
            let client = make_client(transport);
            match client
                .kb_category_create(
                    id,
                    name,
                    parent,
                    description,
                    memory_weight,
                    memory_bucket_id,
                )
                .await
            {
                Ok(cat) => {
                    println!("Created category {} {}", cat.id, cat.name);
                }
                Err(e) => exit_with_error(format!("failed to create category: {e}")),
            }
        }
        crate::cli::CategoryCommands::Delete { id } => {
            let client = make_client(transport);
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

/// Review unrecognized-predicate facts staged by closed taxonomy extraction.
pub async fn handle_kb_staged(
    command: crate::cli::StagedCommands,
    transport: &crate::transport::DaemonTransport,
) {
    match command {
        crate::cli::StagedCommands::List { json } => {
            let client = make_client(transport);
            match client.kb_staged().await {
                Ok(resp) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                        return;
                    }
                    if resp.items.is_empty() {
                        println!("No staged facts.");
                        return;
                    }
                    println!("Staged facts ({}):", resp.total);
                    println!(
                        "{:<8} {:<12} {:<20} {:<24} Source",
                        "ID", "Predicate", "Raw reference", "Updated"
                    );
                    for row in &resp.items {
                        println!(
                            "{:<8} {:<12} {:<20} {:<24} {}",
                            row.id,
                            truncate(&row.relationship_type_raw, 12),
                            truncate(row.raw_reference.as_deref().unwrap_or("-"), 20),
                            row.updated_at,
                            row.connector_instance_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "-".to_string())
                        );
                    }
                    println!(
                        "Use `mimir kb staged map <id> --relationship-type-id <id>` to map a row."
                    );
                }
                Err(e) => exit_with_error(e),
            }
        }
        crate::cli::StagedCommands::Map {
            id,
            relationship_type_id,
            note,
        } => {
            let client = make_client(transport);
            match client
                .kb_staged_map(id, relationship_type_id, note.as_deref())
                .await
            {
                Ok(resp) => println!(
                    "Mapped staged fact {} to leaf {}.",
                    resp.id, resp.relationship_type_id
                ),
                Err(e) => exit_with_error(e),
            }
        }
        crate::cli::StagedCommands::Reject { id, note } => {
            let client = make_client(transport);
            match client.kb_staged_reject(id, note.as_deref()).await {
                Ok(()) => println!("Rejected staged fact {}.", id),
                Err(e) => exit_with_error(e),
            }
        }
    }
}

/// List sensitive facts awaiting confirmation.
pub async fn handle_kb_pending(json: bool, transport: &crate::transport::DaemonTransport) {
    let client = make_client(transport);
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
pub async fn handle_kb_confirm(
    fact_id: i32,
    json: bool,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
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
pub async fn handle_kb_reject(
    fact_id: i32,
    reason: Option<String>,
    transport: &crate::transport::DaemonTransport,
) {
    let client = make_client(transport);
    match client.kb_reject(fact_id, reason.as_deref()).await {
        Ok(()) => println!("Rejected and deleted fact {}.", fact_id),
        Err(e) => exit_with_error(e),
    }
}

// ------------------------------------------------------------------
// kb reset
// ------------------------------------------------------------------

/// Outcome of a completed `kb reset` wipe.
#[derive(Debug)]
pub struct ResetOutcome {
    /// Facts hard-deleted by the daemon (already backed up).
    pub facts_deleted: u64,
    /// Backup file created by the daemon before the wipe.
    pub backup_path: Option<String>,
}

/// Injectable dependencies for the reset flow (tests script the prompt and
/// skip the countdown).
pub(crate) struct ResetFlowDeps<'a> {
    pub prompt: &'a dyn PromptDriver,
    pub countdown_seconds: u64,
}

/// `mimir kb reset` — dedicated full-wipe flow with explicit confirmation.
///
/// Safety lives in the CLI (live counts, exact-phrase prompt, countdown);
/// the daemon still enforces the phrase and creates a backup before the
/// hard delete via the shared `kb forget --all` machinery (issue #69).
pub async fn handle_kb_reset(transport: &crate::transport::DaemonTransport) {
    if !std::io::stdin().is_terminal() {
        exit_with_error(
            "kb reset needs an interactive terminal for its safety confirmation. \
             For a non-interactive full wipe use: mimir kb forget --all --confirmation-phrase \"DELETE EVERYTHING\"",
        );
    }
    let client = make_client(transport);
    let deps = ResetFlowDeps {
        prompt: &InquirePrompt,
        countdown_seconds: 5,
    };
    match run_kb_reset(&client, deps).await {
        Ok(Some(outcome)) => {
            println!(
                "Knowledge Graph wiped. {} facts deleted permanently.",
                outcome.facts_deleted
            );
            if let Some(path) = outcome.backup_path {
                println!("Backup created: {}", path);
            }
        }
        Ok(None) => {
            println!("Aborted: confirmation phrase did not match. No changes were made.")
        }
        Err(e) => exit_with_error(e),
    }
}

/// Run the reset flow against the daemon: warn with live counts, require the
/// exact phrase, count down, then dispatch the shared full-wipe path.
///
/// `Ok(None)` means the user did not confirm; `Ok(Some(_))` reports a
/// completed wipe.
pub(crate) async fn run_kb_reset(
    client: &MimirClient,
    deps: ResetFlowDeps<'_>,
) -> Result<Option<ResetOutcome>, String> {
    let heatmap = client
        .kb_heatmap()
        .await
        .map_err(crate::connector::render_client_error)?;

    println!("⚠️  WARNING: This will permanently delete ALL knowledge.");
    println!(
        "    This includes {} entities and {} non-trashed facts.",
        heatmap.entities, heatmap.facts
    );
    println!("    Trashed facts will also be deleted permanently.");
    println!("    Your configuration, connectors, and system settings will remain.");
    println!();
    println!("    This action CANNOT be undone from the trash bin.");
    println!();

    let phrase = deps
        .prompt
        .input("To confirm, type: DELETE EVERYTHING", None)
        .map_err(|e| e.to_string())?;
    if phrase != "DELETE EVERYTHING" {
        return Ok(None);
    }

    for remaining in (1..=deps.countdown_seconds).rev() {
        println!("Wiping in {remaining}…");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    let resp = client
        .kb_forget(ForgetRequest {
            fact_id: None,
            predicate: None,
            subject: None,
            entity: None,
            source: None,
            from: None,
            to: None,
            all: true,
            yes: false,
            confirm_sensitive: false,
            confirmation_phrase: Some("DELETE EVERYTHING".to_string()),
            archive: false,
        })
        .await
        .map_err(crate::connector::render_client_error)?;

    Ok(Some(ResetOutcome {
        facts_deleted: resp.forgotten_count,
        backup_path: resp.backup_path,
    }))
}
