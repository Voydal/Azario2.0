mod data;
mod generate;
mod metrics;
mod report;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use data::{Result, load};
use report::{RunConfig, sweep, write_reports};

#[derive(Parser)]
#[command(
    version,
    about = "Offline parking perception evaluation and detection artifact generation"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Evaluate(EvaluateArgs),
    Sweep(SweepArgs),
    GenerateDetections(generate::GenerateArgs),
}

#[derive(Args)]
struct CommonArgs {
    #[arg(long)]
    dataset: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long, default_value_t = 250)]
    ground_truth_tolerance_ms: i64,
    #[arg(long, default_value_t = 10_000)]
    transition_timeout_ms: i64,
    #[arg(long)]
    max_false_free_rate: Option<f64>,
    #[arg(long)]
    min_coverage: Option<f64>,
    #[arg(
        long,
        help = "Explicit RFC3339 timestamp for reproducible report metadata"
    )]
    evaluation_timestamp: Option<String>,
}

#[derive(Args)]
struct EvaluateArgs {
    #[command(flatten)]
    common: CommonArgs,
    #[arg(long, default_value_t = 0.5)]
    confidence: f32,
    #[arg(long, default_value_t = 0.1)]
    free: f32,
    #[arg(long, default_value_t = 0.3)]
    occupied: f32,
    #[arg(long, default_value_t = 3)]
    stable_samples: u32,
}

#[derive(Args)]
struct SweepArgs {
    #[command(flatten)]
    common: CommonArgs,
    #[arg(long, value_delimiter = ',', required = true)]
    confidence_values: Vec<f32>,
    #[arg(long, value_delimiter = ',', required = true)]
    free_values: Vec<f32>,
    #[arg(long, value_delimiter = ',', required = true)]
    occupied_values: Vec<f32>,
    #[arg(long, value_delimiter = ',', required = true)]
    stable_samples_values: Vec<u32>,
}

fn validate_common(common: &CommonArgs) -> Result<()> {
    if common.ground_truth_tolerance_ms < 0 || common.transition_timeout_ms < 0 {
        return Err("tolerance and timeout must be non-negative".into());
    }
    for value in [common.max_false_free_rate, common.min_coverage]
        .into_iter()
        .flatten()
    {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err("selection rates must be finite in 0..=1".into());
        }
    }
    if let Some(value) = &common.evaluation_timestamp {
        chrono::DateTime::parse_from_rfc3339(value)?;
    }
    Ok(())
}

fn execute(common: CommonArgs, config: RunConfig, single: bool) -> Result<()> {
    validate_common(&common)?;
    let dataset = load(&common.dataset, config.ground_truth_tolerance_ms)?;
    let run = sweep(&dataset, &config)?;
    write_reports(&dataset, &config, &run, &common.output, single)?;
    println!("Configurations evaluated: {}", run.evaluations.len());
    println!(
        "Invalid combinations skipped: {}",
        run.invalid_combinations_skipped
    );
    if let Some(evaluation) = run.evaluations.first().filter(|_| single) {
        println!("Samples: {}", evaluation.stabilized.labelled_samples);
        println!("Coverage: {:?}", evaluation.stabilized.coverage);
        println!("Macro F1: {:?}", evaluation.stabilized.macro_f1);
        println!(
            "False-free rate: {:?}",
            evaluation.stabilized.false_free_rate
        );
        println!(
            "False-occupied rate: {:?}",
            evaluation.stabilized.false_occupied_rate
        );
        println!(
            "Missed transitions: {}",
            evaluation.transitions.missed_transition_count
        );
        println!(
            "Mean FREE->OCCUPIED latency: {:?}",
            evaluation.transitions.free_to_occupied.mean_latency_ms
        );
        println!(
            "Mean OCCUPIED->FREE latency: {:?}",
            evaluation.transitions.occupied_to_free.mean_latency_ms
        );
    }
    if let Some(index) = run.selected_index {
        println!(
            "Selected configuration: {:?}",
            run.evaluations[index].parameters
        );
    }
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::GenerateDetections(args) => generate::run(args),
        Command::Evaluate(args) => {
            let config = RunConfig {
                ground_truth_tolerance_ms: args.common.ground_truth_tolerance_ms,
                transition_timeout_ms: args.common.transition_timeout_ms,
                confidence_values: vec![args.confidence],
                free_values: vec![args.free],
                occupied_values: vec![args.occupied],
                stable_samples_values: vec![args.stable_samples],
                max_false_free_rate: args.common.max_false_free_rate,
                min_coverage: args.common.min_coverage,
                evaluation_timestamp: args.common.evaluation_timestamp.clone(),
            };
            execute(args.common, config, true)
        }
        Command::Sweep(args) => {
            let config = RunConfig {
                ground_truth_tolerance_ms: args.common.ground_truth_tolerance_ms,
                transition_timeout_ms: args.common.transition_timeout_ms,
                confidence_values: args.confidence_values,
                free_values: args.free_values,
                occupied_values: args.occupied_values,
                stable_samples_values: args.stable_samples_values,
                max_false_free_rate: args.common.max_false_free_rate,
                min_coverage: args.common.min_coverage,
                evaluation_timestamp: args.common.evaluation_timestamp.clone(),
            };
            execute(args.common, config, false)
        }
    }
}
