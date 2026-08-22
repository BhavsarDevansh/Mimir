//! `mimir personality list` — first-class preset discovery (issue #387).
//!
//! Presets are local data: the built-ins are compiled into `mimir-core` and
//! custom presets are plain files in the XDG config directory, so listing
//! them runs entirely in the CLI process and never requires a daemon.

use mimir_core::personality::Personality;

/// List every available preset (built-in + custom) as a table, printing
/// non-fatal diagnostics (malformed preset files, an unknown configured
/// preset) to stderr while still exiting successfully.
pub fn handle_personality_list() {
    let configured_preset = mimir_core::config::Config::load(None)
        .map(|config| config.personality.preset.clone())
        .unwrap_or_else(|_| "transparent".to_string());

    let presets_dir = match mimir_core::paths::personalities_dir() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!(
                "Warning: failed to resolve the personalities directory ({error}); listing built-in presets only."
            );
            // An empty path can never exist, so the scan finds no custom
            // presets and the built-ins still list.
            std::path::PathBuf::new()
        }
    };

    let personality = Personality::from_path(&presets_dir, &configured_preset);

    for warning in personality.warnings() {
        match &warning.path {
            Some(path) => eprintln!("Warning: {} ({})", warning.reason, path.display()),
            None => eprintln!("Warning: {}", warning.reason),
        }
    }

    let presets = personality.list_presets();
    if presets.is_empty() {
        println!("No personality presets found.");
        return;
    }

    use tabled::{Table, Tabled, settings::Style};

    #[derive(Tabled)]
    struct PresetRow {
        #[tabled(rename = "NAME")]
        name: String,
        #[tabled(rename = "SOURCE")]
        source: String,
        #[tabled(rename = "DESCRIPTION")]
        description: String,
    }

    let rows: Vec<PresetRow> = presets
        .iter()
        .map(|preset| PresetRow {
            name: preset.name.clone(),
            source: preset.source.to_string(),
            description: preset
                .description
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        })
        .collect();

    let mut table = Table::new(rows);
    table.with(Style::modern());
    println!("{table}");
}
