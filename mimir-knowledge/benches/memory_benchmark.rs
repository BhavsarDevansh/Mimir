use mimir_knowledge::benchmark::{
    BenchmarkConfig, compare_baseline, load_baseline, run_memory_benchmark, save_baseline,
};

struct BenchmarkCli {
    config: BenchmarkConfig,
    baseline_path: Option<std::path::PathBuf>,
    output_path: Option<std::path::PathBuf>,
    save_baseline_path: Option<std::path::PathBuf>,
}

enum ParsedCli {
    Help,
    Run(BenchmarkCli),
}

fn usage() -> &'static str {
    "Usage: memory_benchmark [--seed SEED] [--scale COUNT] [--baseline PATH] \
     [--output PATH] [--save-baseline PATH]"
}

fn parse_next_value<I>(args: &mut I, flag: &str) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_positive_usize(value: String, flag: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} must be a positive integer"))
        .and_then(|parsed| {
            if parsed == 0 {
                Err(format!("{flag} must be greater than zero"))
            } else {
                Ok(parsed)
            }
        })
}

fn parse_cli<I>(mut args: I) -> Result<ParsedCli, String>
where
    I: Iterator<Item = String>,
{
    let mut cli = BenchmarkCli {
        config: BenchmarkConfig::default(),
        baseline_path: None,
        output_path: None,
        save_baseline_path: None,
    };
    args.next();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" => return Ok(ParsedCli::Help),
            "--bench" => {}
            "--bench-name" => {
                let name = parse_next_value(&mut args, &arg)?;
                if name != "memory_benchmark" {
                    return Err(format!("unknown benchmark: {name}"));
                }
            }
            "--seed" => {
                cli.config.seed = parse_next_value(&mut args, &arg)?
                    .parse()
                    .map_err(|_| "seed must be a u64".to_string())?;
            }
            "--scale" => {
                cli.config.scale_multiplier =
                    parse_positive_usize(parse_next_value(&mut args, &arg)?, &arg)?;
            }
            "--baseline" => {
                cli.baseline_path =
                    Some(std::path::PathBuf::from(parse_next_value(&mut args, &arg)?));
            }
            "--output" => {
                cli.output_path =
                    Some(std::path::PathBuf::from(parse_next_value(&mut args, &arg)?));
            }
            "--save-baseline" => {
                cli.save_baseline_path =
                    Some(std::path::PathBuf::from(parse_next_value(&mut args, &arg)?));
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    Ok(ParsedCli::Run(cli))
}

#[tokio::main]
async fn main() {
    let cli = match parse_cli(std::env::args()) {
        Ok(ParsedCli::Help) => {
            println!("{}", usage());
            std::process::exit(0);
        }
        Ok(ParsedCli::Run(cli)) => cli,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };

    let mut baseline_comparison: Option<mimir_knowledge::benchmark::BaselineComparison> = None;

    let report = match run_memory_benchmark(&cli.config).await {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };

    if let Some(path) = cli.baseline_path {
        match load_baseline(&path).await {
            Ok(baseline) => {
                let comparison = compare_baseline(&report, &baseline);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&comparison)
                        .expect("baseline comparison serialization")
                );
                baseline_comparison = Some(comparison);
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    }

    let json = serde_json::to_string_pretty(&report).expect("report serialization");
    println!("{json}");

    let report_writes = [
        ("baseline", cli.save_baseline_path.as_ref()),
        ("output", cli.output_path.as_ref()),
    ];
    for (label, path) in report_writes
        .into_iter()
        .filter_map(|(label, path)| path.map(|path| (label, path)))
    {
        if let Err(error) = save_baseline(&report, path).await {
            eprintln!("{label} write failed: {error}");
            std::process::exit(1);
        }
    }

    if !report.violations.is_empty()
        || baseline_comparison
            .as_ref()
            .is_some_and(|comparison| !comparison.regressions.is_empty())
    {
        eprintln!("memory benchmark failed: baseline regression or budget violation");
        std::process::exit(1);
    }
}
