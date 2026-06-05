mod ask;
mod chat;
mod cli;
mod commands;
mod constants;
mod daemon_guard;
mod init;
mod kb;
mod memory_cmd;
mod skills_permissions_config;
mod start;
mod status;
mod stop;

use clap::Parser;
use cli::Cli;
use commands::{handle_skill_command, handle_tool_command};
use kb::handle_kb_audit;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mut daemon_started = false;
    let base_url = constants::base_url();

    match cli.command {
        cli::Commands::Tool { command } => handle_tool_command(command).await,
        cli::Commands::Skill { command } => handle_skill_command(command).await,
        cli::Commands::Kb { command } => match command {
            cli::KbCommands::Optimization { status, run_now } => {
                kb::handle_kb_optimization(status, run_now, &base_url).await
            }
            cli::KbCommands::Audit {
                entity,
                predicate,
                from,
                to,
                change_type,
            } => handle_kb_audit(entity, predicate, from, to, change_type).await,
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
                kb::handle_kb_forget(kb::KbForgetInput {
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
                })
                .await
            }
            cli::KbCommands::Restore { trash_id, all } => {
                kb::handle_kb_restore(trash_id, all).await
            }
            cli::KbCommands::Trash {
                empty,
                limit,
                offset,
            } => kb::handle_kb_trash(empty, limit, offset).await,
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
            if let Err(e) =
                daemon_guard::ensure_daemon_running(&base_url, &mut daemon_started).await
            {
                eprintln!("{}", e);
                std::process::exit(1);
            }

            let piped = ask::read_piped_input();
            let query_str = query.join(" ");
            if query_str.trim().is_empty() && piped.is_none() {
                eprintln!("Error: no query provided.");
                std::process::exit(1);
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
        cli::Commands::Chat => {
            if let Err(e) =
                daemon_guard::ensure_daemon_running(&base_url, &mut daemon_started).await
            {
                eprintln!("{}", e);
                std::process::exit(1);
            }
            chat::handle_chat(&base_url).await;
        }
        cli::Commands::Status => {
            if let Err(e) =
                daemon_guard::ensure_daemon_running(&base_url, &mut daemon_started).await
            {
                eprintln!("{}", e);
                std::process::exit(1);
            }
            status::handle_status(&base_url).await;
        }
        cli::Commands::Memory => {
            if let Err(e) =
                daemon_guard::ensure_daemon_running(&base_url, &mut daemon_started).await
            {
                eprintln!("{}", e);
                std::process::exit(1);
            }
            memory_cmd::handle_memory(&base_url).await;
        }
    }
}
