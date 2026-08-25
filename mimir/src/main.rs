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
mod personality_cmd;
mod start;
mod status;
mod stop;
mod transport;

use clap::Parser;
use cli::Cli;
use commands::{handle_skill_command, handle_tool_command};
use transport::DaemonTransport;

/// Ensure the daemon is running, exiting the process on failure.
async fn ensure_daemon(transport: &DaemonTransport, daemon_started: &mut bool) {
    if let Err(e) = daemon_guard::ensure_daemon_running(transport, daemon_started).await {
        commands::exit_with_error(e);
    }
}
#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mut daemon_started = false;
    let transport = DaemonTransport::resolve();

    match cli.command {
        cli::Commands::Tool { command } => handle_tool_command(command).await,
        cli::Commands::Skill { command } => handle_skill_command(command).await,
        cli::Commands::Kb { command } => match command {
            cli::KbCommands::Category { command } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_category(command, &transport).await
            }
            cli::KbCommands::Optimization {
                status,
                run_now,
                json,
            } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_optimization(status, run_now, json, &transport).await
            }
            cli::KbCommands::Query {
                entity,
                predicate,
                min_confidence,
                json,
            } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_query(entity, predicate, min_confidence, json, &transport).await;
            }
            cli::KbCommands::Heatmap { json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_heatmap(json, &transport).await;
            }
            cli::KbCommands::Export { dir, stdout, json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_export(dir, stdout, json, &transport).await;
            }
            cli::KbCommands::Import {
                path,
                dry_run,
                json,
            } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_import(path, dry_run, json, &transport).await;
            }
            cli::KbCommands::Reset => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_reset(&transport).await;
            }
            cli::KbCommands::Show { fact_id, json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_show(fact_id, json, &transport).await;
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
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_edit(
                    fact_id,
                    confidence,
                    valid_from,
                    valid_until,
                    object,
                    status,
                    json,
                    &transport,
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
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_browse(entity, depth, limit, offset, json, &transport).await;
            }
            cli::KbCommands::Profile { entity, json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_profile(entity, json, &transport).await;
            }
            cli::KbCommands::Audit {
                entity,
                predicate,
                from,
                to,
                change_type,
                json,
            } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_audit(entity, predicate, from, to, change_type, json, &transport)
                    .await;
            }
            cli::KbCommands::Merges { command } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                match command {
                    cli::MergeCommands::List { json } => {
                        kb::handle_kb_merges(json, &transport).await
                    }
                    cli::MergeCommands::Apply { id } => {
                        kb::handle_kb_merge_apply(id, &transport).await
                    }
                    cli::MergeCommands::Keep { id } => {
                        kb::handle_kb_merge_keep(id, &transport).await
                    }
                }
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
                ensure_daemon(&transport, &mut daemon_started).await;
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
                    &transport,
                )
                .await;
            }
            cli::KbCommands::Restore { trash_id, all } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_restore(trash_id, all, &transport).await;
            }
            cli::KbCommands::Trash {
                empty,
                limit,
                offset,
                json,
            } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_trash(empty, limit, offset, json, &transport).await;
            }
            cli::KbCommands::Pending { json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_pending(json, &transport).await;
            }
            cli::KbCommands::Confirm { fact_id, json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_confirm(fact_id, json, &transport).await;
            }
            cli::KbCommands::Reject { fact_id, reason } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                kb::handle_kb_reject(fact_id, reason, &transport).await;
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
                password_stdin,
                token,
                token_stdin,
                json,
            } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                match (connector_type, backend) {
                    (Some(connector_type), Some(backend)) => {
                        connector::handle_connector_add(
                            connector_type,
                            backend,
                            config,
                            config_json,
                            slug,
                            name,
                            password,
                            password_stdin,
                            token,
                            token_stdin,
                            json,
                            &transport,
                        )
                        .await;
                    }
                    (None, None) => connector::handle_connector_add_wizard(json, &transport).await,
                    (None, Some(_)) => commands::exit_with_error(
                        "--backend requires a connector type — run `mimir connector add gmail --backend imap` (or just `mimir connector add` for the interactive wizard)",
                    ),
                    (Some(_), None) => commands::exit_with_error(
                        "a connector type requires --backend — run `mimir connector add gmail --backend imap` (or just `mimir connector add` for the interactive wizard)",
                    ),
                }
            }
            cli::ConnectorCommands::List { json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                connector::handle_connector_list(json, &transport).await;
            }
            cli::ConnectorCommands::Auth {
                slug,
                config,
                config_json,
                password,
                password_stdin,
                token,
                token_stdin,
                json,
            } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                connector::handle_connector_auth(
                    slug,
                    config,
                    config_json,
                    password,
                    password_stdin,
                    token,
                    token_stdin,
                    json,
                    &transport,
                )
                .await;
            }
            cli::ConnectorCommands::Catalog { json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                connector::handle_connector_catalog(json, &transport).await;
            }
            cli::ConnectorCommands::Status { slug, json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                connector::handle_connector_status(slug, json, &transport).await;
            }
            cli::ConnectorCommands::Sync {
                slug,
                full,
                since,
                json,
            } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                connector::handle_connector_sync(slug, full, since, json, &transport).await;
            }
            cli::ConnectorCommands::Pause { slug, json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                connector::handle_connector_pause(slug, json, &transport).await;
            }
            cli::ConnectorCommands::Resume { slug, json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                connector::handle_connector_resume(slug, json, &transport).await;
            }
            cli::ConnectorCommands::Remove { slug, yes } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                connector::handle_connector_remove(slug, yes, &transport).await;
            }
            cli::ConnectorCommands::Forget { slug, yes, json } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                connector::handle_connector_forget(slug, yes, json, &transport).await;
            }
            cli::ConnectorCommands::Act {
                slug,
                kind,
                payload,
                json_file,
                json,
            } => {
                ensure_daemon(&transport, &mut daemon_started).await;
                connector::handle_connector_act(slug, kind, payload, json_file, json, &transport)
                    .await;
            }
        },
        cli::Commands::Personality { command } => match command {
            cli::PersonalityCommands::List => personality_cmd::handle_personality_list(),
        },
        cli::Commands::Init => init::handle_init().await,
        cli::Commands::Start => start::handle_start().await,
        cli::Commands::Stop => {
            if !daemon_guard::check_daemon_reachable(&transport).await {
                eprintln!("Mimir is not running.");
                std::process::exit(1);
            }
            stop::handle_stop(&transport).await;
        }
        cli::Commands::Ask {
            query,
            no_stream,
            model,
            verbose,
            incognito,
            personality,
        } => {
            ensure_daemon(&transport, &mut daemon_started).await;

            let piped = ask::read_piped_input();
            let query_str = query.join(" ");
            if query_str.trim().is_empty() && piped.is_none() {
                crate::commands::exit_with_error("no query provided.");
            }
            ask::handle_ask(
                &transport,
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
            ensure_daemon(&transport, &mut daemon_started).await;
            chat::handle_chat(
                &transport,
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
            ensure_daemon(&transport, &mut daemon_started).await;
            status::handle_status(&transport).await;
        }
        cli::Commands::Memory { refresh } => {
            ensure_daemon(&transport, &mut daemon_started).await;
            memory_cmd::handle_memory(&transport, refresh).await;
        }
    }
}
