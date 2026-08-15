#![deny(unsafe_code)]
mod ask;
mod chat;
mod cli;
mod cli_util;
mod commands;
mod connector;
mod constants;
mod daemon_guard;
mod init;
mod kb;
mod memory_cmd;
mod start;
mod status;
mod stop;

use clap::Parser;
use cli::Cli;
use commands::{handle_skill_command, handle_tool_command};

/// Ensure the daemon is running, exiting the process on failure.
async fn ensure_daemon(base_url: &str, daemon_started: &mut bool) {
    if let Err(e) = daemon_guard::ensure_daemon_running(base_url, daemon_started).await {
        commands::exit_with_error(e);
    }
}
#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mut daemon_started = false;
    let base_url = constants::base_url();

    match cli.command {
        cli::Commands::Tool { command } => handle_tool_command(command).await,
        cli::Commands::Skill { command } => handle_skill_command(command).await,
        cli::Commands::Kb { command } => match command {
            cli::KbCommands::Category { command } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_category(command, &base_url).await
            }
            cli::KbCommands::Optimization {
                status,
                run_now,
                json,
            } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_optimization(status, run_now, json, &base_url).await
            }
            cli::KbCommands::Query {
                entity,
                predicate,
                min_confidence,
                json,
            } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_query(entity, predicate, min_confidence, json, &base_url).await;
            }
            cli::KbCommands::Show { fact_id, json } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_show(fact_id, json, &base_url).await;
            }
            cli::KbCommands::Edit {
                fact_id,
                confidence,
                valid_from,
                valid_until,
                object,
                status,
                json,
            } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_edit(
                    fact_id,
                    confidence,
                    valid_from,
                    valid_until,
                    object,
                    status,
                    json,
                    &base_url,
                )
                .await;
            }
            cli::KbCommands::Browse {
                entity,
                depth,
                limit,
                offset,
                json,
            } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_browse(entity, depth, limit, offset, json, &base_url).await;
            }
            cli::KbCommands::Profile { entity, json } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_profile(entity, json, &base_url).await;
            }
            cli::KbCommands::Audit {
                entity,
                predicate,
                from,
                to,
                change_type,
                json,
            } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_audit(entity, predicate, from, to, change_type, json, &base_url)
                    .await;
            }
            cli::KbCommands::Forget {
                fact_id,
                predicate,
                subject,
                entity,
                source,
                from,
                to,
                all,
                yes,
                confirm_sensitive,
                archive,
                confirmation_phrase,
            } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_forget(
                    kb::KbForgetInput {
                        fact_id,
                        predicate,
                        subject,
                        entity,
                        source,
                        from,
                        to,
                        all,
                        yes,
                        confirm_sensitive,
                        archive,
                        confirmation_phrase,
                    },
                    &base_url,
                )
                .await;
            }
            cli::KbCommands::Restore { trash_id, all } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_restore(trash_id, all, &base_url).await;
            }
            cli::KbCommands::Trash {
                empty,
                limit,
                offset,
                json,
            } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_trash(empty, limit, offset, json, &base_url).await;
            }
            cli::KbCommands::Pending { json } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_pending(json, &base_url).await;
            }
            cli::KbCommands::Confirm { fact_id, json } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_confirm(fact_id, json, &base_url).await;
            }
            cli::KbCommands::Reject { fact_id, reason } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                kb::handle_kb_reject(fact_id, reason, &base_url).await;
            }
        },
        cli::Commands::Connector { command } => match command {
            cli::ConnectorCommands::Add {
                connector_type,
                backend,
                config,
                config_json,
                slug,
                name,
                password,
                token,
                json,
            } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                connector::handle_connector_add(
                    connector_type,
                    backend,
                    config,
                    config_json,
                    slug,
                    name,
                    password,
                    token,
                    json,
                    &base_url,
                )
                .await;
            }
            cli::ConnectorCommands::List { json } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                connector::handle_connector_list(json, &base_url).await;
            }
            cli::ConnectorCommands::Auth {
                slug,
                config,
                config_json,
                password,
                token,
                json,
            } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                connector::handle_connector_auth(
                    slug,
                    config,
                    config_json,
                    password,
                    token,
                    json,
                    &base_url,
                )
                .await;
            }
            cli::ConnectorCommands::Catalog { json } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                connector::handle_connector_catalog(json, &base_url).await;
            }
            cli::ConnectorCommands::Status { slug, json } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                connector::handle_connector_status(slug, json, &base_url).await;
            }
            cli::ConnectorCommands::Sync {
                slug,
                full,
                since,
                json,
            } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                connector::handle_connector_sync(slug, full, since, json, &base_url).await;
            }
            cli::ConnectorCommands::Pause { slug, json } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                connector::handle_connector_pause(slug, json, &base_url).await;
            }
            cli::ConnectorCommands::Resume { slug, json } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                connector::handle_connector_resume(slug, json, &base_url).await;
            }
            cli::ConnectorCommands::Remove { slug, yes } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                connector::handle_connector_remove(slug, yes, &base_url).await;
            }
            cli::ConnectorCommands::Forget { slug, yes, json } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                connector::handle_connector_forget(slug, yes, json, &base_url).await;
            }
            cli::ConnectorCommands::Act {
                slug,
                kind,
                payload,
                json_file,
                json,
            } => {
                ensure_daemon(&base_url, &mut daemon_started).await;
                connector::handle_connector_act(slug, kind, payload, json_file, json, &base_url)
                    .await;
            }
        },
        cli::Commands::Init => init::handle_init().await,
        cli::Commands::Start => start::handle_start().await,
        cli::Commands::Stop => {
            if !daemon_guard::check_daemon_reachable(&base_url).await {
                eprintln!("Mimir is not running.");
                std::process::exit(1);
            }
            stop::handle_stop(&base_url).await;
        }
        cli::Commands::Ask {
            query,
            no_stream,
            model,
            verbose,
            incognito,
            personality,
        } => {
            ensure_daemon(&base_url, &mut daemon_started).await;

            let piped = ask::read_piped_input();
            let query_str = query.join(" ");
            if query_str.trim().is_empty() && piped.is_none() {
                crate::commands::exit_with_error("no query provided.");
            }
            ask::handle_ask(
                &base_url,
                ask::AskOptions {
                    query: query_str,
                    no_stream,
                    model,
                    verbose,
                    incognito,
                    personality,
                    piped_input: piped,
                },
            )
            .await;
        }
        cli::Commands::Chat {
            model,
            verbose,
            incognito,
            personality,
        } => {
            ensure_daemon(&base_url, &mut daemon_started).await;
            chat::handle_chat(
                &base_url,
                chat::ChatOptions {
                    model,
                    verbose,
                    incognito,
                    personality,
                },
            )
            .await;
        }
        cli::Commands::Status => {
            ensure_daemon(&base_url, &mut daemon_started).await;
            status::handle_status(&base_url).await;
        }
        cli::Commands::Memory { refresh } => {
            ensure_daemon(&base_url, &mut daemon_started).await;
            memory_cmd::handle_memory(&base_url, refresh).await;
        }
    }
}
