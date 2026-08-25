//! KB entity merge-queue review handlers (issue #282): list pending
//! suggestions, apply a merge, or keep the pair separate.

use super::{exit_with_error, make_client};
use crate::transport::DaemonTransport;

pub async fn handle_kb_merges(json: bool, transport: &DaemonTransport) {
    let client = make_client(transport);
    match client.kb_merges().await {
        Ok(resp) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                return;
            }
            if resp.items.is_empty() {
                println!("No pending entity merges.");
                return;
            }
            use tabled::{Table, Tabled, settings::Style};
            #[derive(Tabled)]
            struct MergeTableRow {
                id: i64,
                primary: String,
                primary_type: String,
                duplicate: String,
                duplicate_type: String,
                suggested: String,
                confidence: String,
                queued_at: String,
            }
            let rows: Vec<MergeTableRow> = resp
                .items
                .iter()
                .map(|i| MergeTableRow {
                    id: i.id,
                    primary: i.primary_name.clone(),
                    primary_type: i.primary_type.clone(),
                    duplicate: i.duplicate_name.clone(),
                    duplicate_type: i.duplicate_type.clone(),
                    suggested: i.suggested_action.clone().unwrap_or_default(),
                    confidence: i
                        .llm_confidence
                        .map(|c| format!("{c:.2}"))
                        .unwrap_or_default(),
                    queued_at: i.queued_at.clone(),
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

pub async fn handle_kb_merge_apply(id: i64, transport: &DaemonTransport) {
    let client = make_client(transport);
    match client.kb_merge_apply(id).await {
        Ok(resp) => {
            println!(
                "Merged entity {} into {}.",
                resp.merged_id, resp.survivor_id
            );
        }
        Err(e) => exit_with_error(e),
    }
}

pub async fn handle_kb_merge_keep(id: i64, transport: &DaemonTransport) {
    let client = make_client(transport);
    match client.kb_merge_keep(id).await {
        Ok(()) => println!("Marked merge {} as kept separate.", id),
        Err(e) => exit_with_error(e),
    }
}
