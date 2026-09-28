use std::{cmp::Ordering, collections::BTreeMap, fs, path::Path};

use serde::Serialize;

use crate::{
    data::{Dataset, Result},
    metrics::{Evaluation, Parameters, evaluate},
};

#[derive(Clone, Debug, Serialize)]
pub struct RunConfig {
    pub ground_truth_tolerance_ms: i64,
    pub transition_timeout_ms: i64,
    pub confidence_values: Vec<f32>,
    pub free_values: Vec<f32>,
    pub occupied_values: Vec<f32>,
    pub stable_samples_values: Vec<u32>,
    pub max_false_free_rate: Option<f64>,
    pub min_coverage: Option<f64>,
    pub evaluation_timestamp: Option<String>,
}

#[derive(Serialize)]
struct Summary<'a> {
    report_schema_version: u32,
    evaluator_version: &'static str,
    evaluation_timestamp: &'a Option<String>,
    dataset_name: &'a str,
    dataset_version: &'a str,
    dataset_description: &'a str,
    model_name: &'a Option<String>,
    model_version: &'a Option<String>,
    model_hash: &'a Option<String>,
    input_sha256: &'a BTreeMap<String, String>,
    number_of_videos: usize,
    number_of_spots: usize,
    prediction_sample_count: usize,
    ground_truth_free_count: usize,
    ground_truth_occupied_count: usize,
    ground_truth_unknown_count: usize,
    input_was_reordered: bool,
    invalid_combinations_skipped: usize,
    configurations_evaluated: usize,
    selected_configuration: Option<&'a Evaluation>,
    single_evaluation: Option<&'a Evaluation>,
    note: &'static str,
}

#[derive(Serialize)]
struct CsvRow {
    detection_confidence_threshold: f32,
    free_threshold: f32,
    occupied_threshold: f32,
    stable_samples_required: u32,
    frame_coverage: Option<f64>,
    frame_precision_free: Option<f64>,
    frame_recall_free: Option<f64>,
    frame_f1_free: Option<f64>,
    frame_precision_occupied: Option<f64>,
    frame_recall_occupied: Option<f64>,
    frame_f1_occupied: Option<f64>,
    frame_macro_f1: Option<f64>,
    frame_false_free_rate: Option<f64>,
    frame_false_occupied_rate: Option<f64>,
    frame_uncertain_on_free_rate: Option<f64>,
    frame_uncertain_on_occupied_rate: Option<f64>,
    labelled_samples: u64,
    excluded_unknown_count: u64,
    unlabelled_prediction_count: u64,
    ground_truth_free_count: u64,
    ground_truth_occupied_count: u64,
    ground_truth_unknown_count: u64,
    coverage: Option<f64>,
    precision_free: Option<f64>,
    recall_free: Option<f64>,
    f1_free: Option<f64>,
    precision_occupied: Option<f64>,
    recall_occupied: Option<f64>,
    f1_occupied: Option<f64>,
    macro_f1: Option<f64>,
    strict_accuracy: Option<f64>,
    false_free_count: u64,
    false_free_rate: Option<f64>,
    false_occupied_count: u64,
    false_occupied_rate: Option<f64>,
    uncertain_on_free_rate: Option<f64>,
    uncertain_on_occupied_rate: Option<f64>,
    transition_count: u64,
    detected_transition_count: u64,
    missed_transition_count: u64,
    free_to_occupied_mean_latency_ms: Option<f64>,
    free_to_occupied_median_latency_ms: Option<f64>,
    free_to_occupied_p95_latency_ms: Option<f64>,
    occupied_to_free_mean_latency_ms: Option<f64>,
    occupied_to_free_median_latency_ms: Option<f64>,
    occupied_to_free_p95_latency_ms: Option<f64>,
    stable_state_changes_count: u64,
    extra_state_changes: u64,
}

impl From<&Evaluation> for CsvRow {
    fn from(e: &Evaluation) -> Self {
        let m = &e.stabilized;
        let t = &e.transitions;
        Self {
            detection_confidence_threshold: e.parameters.detection_confidence_threshold,
            free_threshold: e.parameters.free_threshold,
            occupied_threshold: e.parameters.occupied_threshold,
            stable_samples_required: e.parameters.stable_samples_required,
            frame_coverage: e.frame.coverage,
            frame_precision_free: e.frame.precision_free,
            frame_recall_free: e.frame.recall_free,
            frame_f1_free: e.frame.f1_free,
            frame_precision_occupied: e.frame.precision_occupied,
            frame_recall_occupied: e.frame.recall_occupied,
            frame_f1_occupied: e.frame.f1_occupied,
            frame_macro_f1: e.frame.macro_f1,
            frame_false_free_rate: e.frame.false_free_rate,
            frame_false_occupied_rate: e.frame.false_occupied_rate,
            frame_uncertain_on_free_rate: e.frame.uncertain_on_free_rate,
            frame_uncertain_on_occupied_rate: e.frame.uncertain_on_occupied_rate,
            labelled_samples: m.labelled_samples,
            excluded_unknown_count: m.excluded_unknown_count,
            unlabelled_prediction_count: m.unlabelled_prediction_count,
            ground_truth_free_count: m.ground_truth_free_count,
            ground_truth_occupied_count: m.ground_truth_occupied_count,
            ground_truth_unknown_count: m.ground_truth_unknown_count,
            coverage: m.coverage,
            precision_free: m.precision_free,
            recall_free: m.recall_free,
            f1_free: m.f1_free,
            precision_occupied: m.precision_occupied,
            recall_occupied: m.recall_occupied,
            f1_occupied: m.f1_occupied,
            macro_f1: m.macro_f1,
            strict_accuracy: m.strict_accuracy,
            false_free_count: m.false_free_count,
            false_free_rate: m.false_free_rate,
            false_occupied_count: m.false_occupied_count,
            false_occupied_rate: m.false_occupied_rate,
            uncertain_on_free_rate: m.uncertain_on_free_rate,
            uncertain_on_occupied_rate: m.uncertain_on_occupied_rate,
            transition_count: t.transition_count,
            detected_transition_count: t.detected_transition_count,
            missed_transition_count: t.missed_transition_count,
            free_to_occupied_mean_latency_ms: t.free_to_occupied.mean_latency_ms,
            free_to_occupied_median_latency_ms: t.free_to_occupied.median_latency_ms,
            free_to_occupied_p95_latency_ms: t.free_to_occupied.p95_latency_ms,
            occupied_to_free_mean_latency_ms: t.occupied_to_free.mean_latency_ms,
            occupied_to_free_median_latency_ms: t.occupied_to_free.median_latency_ms,
            occupied_to_free_p95_latency_ms: t.occupied_to_free.p95_latency_ms,
            stable_state_changes_count: t.stable_state_changes_count,
            extra_state_changes: t.extra_state_changes,
        }
    }
}
#[derive(Serialize)]
struct CameraCsvRow {
    camera_id: uuid::Uuid,
    detection_confidence_threshold: f32,
    free_threshold: f32,
    occupied_threshold: f32,
    stable_samples_required: u32,
    labelled_samples: u64,
    coverage: Option<f64>,
    false_free_rate: Option<f64>,
    false_occupied_rate: Option<f64>,
    macro_f1: Option<f64>,
}

pub struct RunResult {
    pub evaluations: Vec<Evaluation>,
    pub invalid_combinations_skipped: usize,
    pub selected_index: Option<usize>,
}

pub fn sweep(dataset: &Dataset, config: &RunConfig) -> Result<RunResult> {
    let mut evaluations = Vec::new();
    let mut invalid = 0;
    for &confidence in &config.confidence_values {
        for &free in &config.free_values {
            for &occupied in &config.occupied_values {
                for &stable in &config.stable_samples_values {
                    if free >= occupied {
                        invalid += 1;
                        continue;
                    }
                    let parameters = Parameters {
                        detection_confidence_threshold: confidence,
                        free_threshold: free,
                        occupied_threshold: occupied,
                        stable_samples_required: stable,
                    };
                    evaluations.push(evaluate(dataset, parameters, config.transition_timeout_ms)?);
                }
            }
        }
    }
    let selected_index = select(
        &evaluations,
        config.max_false_free_rate,
        config.min_coverage,
    );
    Ok(RunResult {
        evaluations,
        invalid_combinations_skipped: invalid,
        selected_index,
    })
}

fn descending(left: Option<f64>, right: Option<f64>) -> Ordering {
    right.partial_cmp(&left).unwrap_or(Ordering::Equal)
}
fn ascending(left: Option<f64>, right: Option<f64>) -> Ordering {
    match (left, right) {
        (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
fn latency(e: &Evaluation) -> Option<f64> {
    let a = e.transitions.free_to_occupied.mean_latency_ms;
    let b = e.transitions.occupied_to_free.mean_latency_ms;
    match (a, b) {
        (Some(x), Some(y)) => Some((x + y) / 2.0),
        (Some(x), None) | (None, Some(x)) => Some(x),
        _ => None,
    }
}

pub fn select(
    evaluations: &[Evaluation],
    max_false_free_rate: Option<f64>,
    min_coverage: Option<f64>,
) -> Option<usize> {
    if max_false_free_rate.is_none() && min_coverage.is_none() {
        return None;
    }
    evaluations
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            max_false_free_rate.is_none_or(|limit| {
                e.stabilized
                    .false_free_rate
                    .is_some_and(|rate| rate <= limit)
            }) && min_coverage.is_none_or(|limit| {
                e.stabilized
                    .coverage
                    .is_some_and(|coverage| coverage >= limit)
            }) && e.stabilized.macro_f1.is_some()
        })
        .min_by(|(_, a), (_, b)| {
            descending(a.stabilized.macro_f1, b.stabilized.macro_f1)
                .then_with(|| ascending(a.stabilized.false_free_rate, b.stabilized.false_free_rate))
                .then_with(|| descending(a.stabilized.coverage, b.stabilized.coverage))
                .then_with(|| ascending(latency(a), latency(b)))
                .then_with(|| {
                    a.parameters
                        .detection_confidence_threshold
                        .total_cmp(&b.parameters.detection_confidence_threshold)
                })
                .then_with(|| {
                    a.parameters
                        .free_threshold
                        .total_cmp(&b.parameters.free_threshold)
                })
                .then_with(|| {
                    a.parameters
                        .occupied_threshold
                        .total_cmp(&b.parameters.occupied_threshold)
                })
                .then_with(|| {
                    a.parameters
                        .stable_samples_required
                        .cmp(&b.parameters.stable_samples_required)
                })
        })
        .map(|(index, _)| index)
}

pub fn write_reports(
    dataset: &Dataset,
    config: &RunConfig,
    run: &RunResult,
    output: &Path,
    single: bool,
) -> Result<()> {
    fs::create_dir_all(output)?;
    fs::write(
        output.join("evaluation-config.json"),
        serde_json::to_vec_pretty(config)?,
    )?;
    let mut csv = csv::Writer::from_path(output.join("results.csv"))?;
    for evaluation in &run.evaluations {
        csv.serialize(CsvRow::from(evaluation))?;
    }
    csv.flush()?;
    let mut camera_csv = csv::Writer::from_path(output.join("per_camera.csv"))?;
    for evaluation in &run.evaluations {
        for (camera_id, metrics) in &evaluation.per_camera {
            camera_csv.serialize(CameraCsvRow {
                camera_id: *camera_id,
                detection_confidence_threshold: evaluation
                    .parameters
                    .detection_confidence_threshold,
                free_threshold: evaluation.parameters.free_threshold,
                occupied_threshold: evaluation.parameters.occupied_threshold,
                stable_samples_required: evaluation.parameters.stable_samples_required,
                labelled_samples: metrics.labelled_samples,
                coverage: metrics.coverage,
                false_free_rate: metrics.false_free_rate,
                false_occupied_rate: metrics.false_occupied_rate,
                macro_f1: metrics.macro_f1,
            })?;
        }
    }
    camera_csv.flush()?;
    let summary = Summary {
        report_schema_version: 1,
        evaluator_version: env!("CARGO_PKG_VERSION"),
        evaluation_timestamp: &config.evaluation_timestamp,
        dataset_name: &dataset.manifest.dataset_name,
        dataset_version: &dataset.manifest.version,
        dataset_description: &dataset.manifest.description,
        model_name: &dataset.manifest.model_name,
        model_version: &dataset.manifest.model_version,
        model_hash: &dataset.manifest.model_hash,
        input_sha256: &dataset.hashes,
        number_of_videos: dataset.video_count,
        number_of_spots: dataset.spot_count,
        prediction_sample_count: dataset.samples.len(),
        ground_truth_free_count: dataset
            .truth_records
            .values()
            .flatten()
            .filter(|(_, state)| *state == crate::data::Truth::Free)
            .count(),
        ground_truth_occupied_count: dataset
            .truth_records
            .values()
            .flatten()
            .filter(|(_, state)| *state == crate::data::Truth::Occupied)
            .count(),
        ground_truth_unknown_count: dataset
            .truth_records
            .values()
            .flatten()
            .filter(|(_, state)| *state == crate::data::Truth::Unknown)
            .count(),
        input_was_reordered: dataset.input_was_reordered,
        invalid_combinations_skipped: run.invalid_combinations_skipped,
        configurations_evaluated: run.evaluations.len(),
        selected_configuration: run.selected_index.map(|index| &run.evaluations[index]),
        single_evaluation: if single {
            run.evaluations.first()
        } else {
            None
        },
        note: "Accuracy can mislead on imbalanced data; inspect false-free, coverage and class balance.",
    };
    fs::write(
        output.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{ClassificationMetrics, TransitionMetrics};

    fn evaluation(f1: f64, false_free: f64, coverage: f64) -> Evaluation {
        Evaluation {
            parameters: Parameters {
                detection_confidence_threshold: 0.5,
                free_threshold: 0.1,
                occupied_threshold: 0.3,
                stable_samples_required: 1,
            },
            frame: ClassificationMetrics::default(),
            stabilized: ClassificationMetrics {
                macro_f1: Some(f1),
                false_free_rate: Some(false_free),
                coverage: Some(coverage),
                ..ClassificationMetrics::default()
            },
            transitions: TransitionMetrics::default(),
            per_camera: BTreeMap::<uuid::Uuid, ClassificationMetrics>::new(),
        }
    }

    #[test]
    fn selection_respects_false_free_constraint() {
        let values = [evaluation(0.9, 0.1, 0.9), evaluation(0.7, 0.01, 0.8)];
        assert_eq!(select(&values, Some(0.02), None), Some(1));
    }

    #[test]
    fn no_selection_without_constraints() {
        assert_eq!(select(&[evaluation(0.9, 0.0, 1.0)], None, None), None);
    }

    #[test]
    fn threshold_sweep_evaluates_valid_combinations() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/perception-eval/dataset.toml");
        let dataset = crate::data::load(&path, 250).unwrap();
        let config = RunConfig {
            ground_truth_tolerance_ms: 250,
            transition_timeout_ms: 10_000,
            confidence_values: vec![0.4, 0.5],
            free_values: vec![0.1, 0.3],
            occupied_values: vec![0.3],
            stable_samples_values: vec![1, 2],
            max_false_free_rate: None,
            min_coverage: None,
            evaluation_timestamp: None,
        };
        let first = sweep(&dataset, &config).unwrap();
        let second = sweep(&dataset, &config).unwrap();
        assert_eq!(first.evaluations.len(), 4);
        assert_eq!(first.invalid_combinations_skipped, 4);
        assert!(first.selected_index.is_none());
        assert_eq!(
            serde_json::to_value(&first.evaluations).unwrap(),
            serde_json::to_value(&second.evaluations).unwrap()
        );
    }
}
