use std::collections::BTreeMap;

use chrono::{Duration, TimeZone, Utc};
use parking_domain::ObservedState;
use parking_perception::{OccupancyConfig, OccupancyEngine, SpotStabilizer};
use serde::Serialize;
use uuid::Uuid;

use crate::data::{AnyError, Dataset, Result, Truth};

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Parameters {
    pub detection_confidence_threshold: f32,
    pub free_threshold: f32,
    pub occupied_threshold: f32,
    pub stable_samples_required: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Confusion {
    pub pred_free_truth_free: u64,
    pub pred_free_truth_occupied: u64,
    pub pred_occupied_truth_free: u64,
    pub pred_occupied_truth_occupied: u64,
    pub pred_uncertain_truth_free: u64,
    pub pred_uncertain_truth_occupied: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClassificationMetrics {
    pub confusion: Confusion,
    pub labelled_samples: u64,
    pub excluded_unknown_count: u64,
    pub unlabelled_prediction_count: u64,
    pub ground_truth_free_count: u64,
    pub ground_truth_occupied_count: u64,
    pub ground_truth_unknown_count: u64,
    pub coverage: Option<f64>,
    pub strict_accuracy: Option<f64>,
    pub precision_free: Option<f64>,
    pub recall_free: Option<f64>,
    pub f1_free: Option<f64>,
    pub precision_occupied: Option<f64>,
    pub recall_occupied: Option<f64>,
    pub f1_occupied: Option<f64>,
    pub macro_f1: Option<f64>,
    pub false_free_count: u64,
    pub false_free_rate: Option<f64>,
    pub false_occupied_count: u64,
    pub false_occupied_rate: Option<f64>,
    pub uncertain_on_free_rate: Option<f64>,
    pub uncertain_on_occupied_rate: Option<f64>,
}

#[derive(Clone, Copy)]
pub struct Point {
    pub truth: Option<Truth>,
    pub prediction: ObservedState,
}

fn ratio(numerator: u64, denominator: u64) -> Option<f64> {
    (denominator != 0).then_some(numerator as f64 / denominator as f64)
}

fn f1(precision: Option<f64>, recall: Option<f64>) -> Option<f64> {
    let (p, r) = (precision?, recall?);
    if p + r == 0.0 {
        Some(0.0)
    } else {
        Some(2.0 * p * r / (p + r))
    }
}

pub fn classify(points: impl IntoIterator<Item = Point>) -> ClassificationMetrics {
    let mut result = ClassificationMetrics::default();
    for point in points {
        match point.truth {
            None => result.unlabelled_prediction_count += 1,
            Some(Truth::Unknown) => {
                result.excluded_unknown_count += 1;
                result.ground_truth_unknown_count += 1;
            }
            Some(Truth::Free) => {
                result.labelled_samples += 1;
                result.ground_truth_free_count += 1;
                match point.prediction {
                    ObservedState::Free => result.confusion.pred_free_truth_free += 1,
                    ObservedState::Occupied => result.confusion.pred_occupied_truth_free += 1,
                    ObservedState::Uncertain => result.confusion.pred_uncertain_truth_free += 1,
                }
            }
            Some(Truth::Occupied) => {
                result.labelled_samples += 1;
                result.ground_truth_occupied_count += 1;
                match point.prediction {
                    ObservedState::Free => result.confusion.pred_free_truth_occupied += 1,
                    ObservedState::Occupied => result.confusion.pred_occupied_truth_occupied += 1,
                    ObservedState::Uncertain => result.confusion.pred_uncertain_truth_occupied += 1,
                }
            }
        }
    }
    let c = &result.confusion;
    let useful = c.pred_free_truth_free
        + c.pred_free_truth_occupied
        + c.pred_occupied_truth_free
        + c.pred_occupied_truth_occupied;
    let correct = c.pred_free_truth_free + c.pred_occupied_truth_occupied;
    result.coverage = ratio(useful, result.labelled_samples);
    result.strict_accuracy = ratio(correct, result.labelled_samples);
    result.precision_free = ratio(
        c.pred_free_truth_free,
        c.pred_free_truth_free + c.pred_free_truth_occupied,
    );
    result.recall_free = ratio(c.pred_free_truth_free, result.ground_truth_free_count);
    result.f1_free = f1(result.precision_free, result.recall_free);
    result.precision_occupied = ratio(
        c.pred_occupied_truth_occupied,
        c.pred_occupied_truth_free + c.pred_occupied_truth_occupied,
    );
    result.recall_occupied = ratio(
        c.pred_occupied_truth_occupied,
        result.ground_truth_occupied_count,
    );
    result.f1_occupied = f1(result.precision_occupied, result.recall_occupied);
    result.macro_f1 = match (result.f1_free, result.f1_occupied) {
        (Some(free), Some(occupied)) => Some((free + occupied) / 2.0),
        _ => None,
    };
    result.false_free_count = c.pred_free_truth_occupied;
    result.false_free_rate = ratio(result.false_free_count, result.ground_truth_occupied_count);
    result.false_occupied_count = c.pred_occupied_truth_free;
    result.false_occupied_rate = ratio(result.false_occupied_count, result.ground_truth_free_count);
    result.uncertain_on_free_rate =
        ratio(c.pred_uncertain_truth_free, result.ground_truth_free_count);
    result.uncertain_on_occupied_rate = ratio(
        c.pred_uncertain_truth_occupied,
        result.ground_truth_occupied_count,
    );
    result
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LatencyMetrics {
    pub transition_count: u64,
    pub detected_transition_count: u64,
    pub missed_transition_count: u64,
    pub mean_latency_ms: Option<f64>,
    pub median_latency_ms: Option<f64>,
    pub p95_latency_ms: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TransitionMetrics {
    pub free_to_occupied: LatencyMetrics,
    pub occupied_to_free: LatencyMetrics,
    pub transition_count: u64,
    pub detected_transition_count: u64,
    pub missed_transition_count: u64,
    pub stable_state_changes_count: u64,
    pub extra_state_changes: u64,
}

#[derive(Clone, Debug)]
pub struct EvaluatedPoint {
    pub video_id: String,
    pub camera_id: Uuid,
    pub spot_id: Uuid,
    pub timestamp_ms: i64,
    pub truth: Option<Truth>,
    pub frame: ObservedState,
    pub stabilized: ObservedState,
    pub stable_established: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Evaluation {
    pub parameters: Parameters,
    pub frame: ClassificationMetrics,
    pub stabilized: ClassificationMetrics,
    pub transitions: TransitionMetrics,
    pub per_camera: BTreeMap<Uuid, ClassificationMetrics>,
}

pub fn evaluate(dataset: &Dataset, parameters: Parameters, timeout_ms: i64) -> Result<Evaluation> {
    if parameters.stable_samples_required == 0 || timeout_ms < 0 {
        return Err(
            "stable_samples_required must be positive and transition_timeout_ms non-negative"
                .into(),
        );
    }
    let config = OccupancyConfig::new(
        dataset.vehicle_class_ids.iter().copied(),
        parameters.detection_confidence_threshold,
        parameters.free_threshold,
        parameters.occupied_threshold,
    )?;
    let engine = OccupancyEngine::new(config);
    let mut stabilizers: BTreeMap<(String, Uuid), SpotStabilizer> = BTreeMap::new();
    let mut points = Vec::with_capacity(dataset.samples.len());
    let refresh = i64::try_from(dataset.refresh_interval_ms)
        .map_err(|_| "observation_refresh_interval_ms too large")?;
    for sample in &dataset.samples {
        let key = (sample.video_id.clone(), sample.spot_id);
        let roi = dataset.rois.get(&key).ok_or("sample has no ROI")?;
        let frame = engine.classify(roi, &sample.detections).state;
        let stabilizer = stabilizers.entry(key).or_insert_with(|| {
            SpotStabilizer::new(
                parameters.stable_samples_required,
                Duration::milliseconds(refresh),
            )
        });
        let now = Utc
            .timestamp_millis_opt(sample.timestamp_ms)
            .single()
            .ok_or_else(|| -> AnyError { "timestamp_ms outside supported range".into() })?;
        stabilizer.observe(frame, now);
        let stable = stabilizer.stable_state();
        points.push(EvaluatedPoint {
            video_id: sample.video_id.clone(),
            camera_id: sample.camera_id,
            spot_id: sample.spot_id,
            timestamp_ms: sample.timestamp_ms,
            truth: sample.truth,
            frame,
            stabilized: stable.unwrap_or(ObservedState::Uncertain),
            stable_established: stable.is_some(),
        });
    }
    let frame = classify(points.iter().map(|p| Point {
        truth: p.truth,
        prediction: p.frame,
    }));
    let stabilized = classify(points.iter().map(|p| Point {
        truth: p.truth,
        prediction: p.stabilized,
    }));
    let mut per_camera_points: BTreeMap<Uuid, Vec<Point>> = BTreeMap::new();
    for point in &points {
        per_camera_points
            .entry(point.camera_id)
            .or_default()
            .push(Point {
                truth: point.truth,
                prediction: point.stabilized,
            });
    }
    let per_camera = per_camera_points
        .into_iter()
        .map(|(id, values)| (id, classify(values)))
        .collect();
    let transitions = transition_metrics(&dataset.truth_records, &points, timeout_ms);
    Ok(Evaluation {
        parameters,
        frame,
        stabilized,
        transitions,
        per_camera,
    })
}

fn finish_latencies(metrics: &mut LatencyMetrics, latencies: &mut [i64]) {
    if latencies.is_empty() {
        return;
    }
    latencies.sort_unstable();
    metrics.mean_latency_ms =
        Some(latencies.iter().map(|v| *v as f64).sum::<f64>() / latencies.len() as f64);
    let middle = latencies.len() / 2;
    metrics.median_latency_ms = Some(if latencies.len().is_multiple_of(2) {
        (latencies[middle - 1] as f64 + latencies[middle] as f64) / 2.0
    } else {
        latencies[middle] as f64
    });
    let rank = (95 * latencies.len()).div_ceil(100).saturating_sub(1);
    metrics.p95_latency_ms = Some(latencies[rank] as f64);
}

pub fn transition_metrics(
    truths: &BTreeMap<(String, Uuid), Vec<(i64, Truth)>>,
    points: &[EvaluatedPoint],
    timeout_ms: i64,
) -> TransitionMetrics {
    let mut result = TransitionMetrics::default();
    let mut predictions: BTreeMap<(String, Uuid), Vec<&EvaluatedPoint>> = BTreeMap::new();
    for point in points {
        predictions
            .entry((point.video_id.clone(), point.spot_id))
            .or_default()
            .push(point);
    }
    let mut occupied_latencies = Vec::new();
    let mut free_latencies = Vec::new();
    for samples in predictions.values() {
        let mut last_stable: Option<ObservedState> = None;
        for sample in samples {
            if sample.stable_established {
                if last_stable.is_some_and(|last| last != sample.stabilized) {
                    result.stable_state_changes_count += 1;
                }
                last_stable = Some(sample.stabilized);
            }
        }
    }
    for (key, annotations) in truths {
        let samples = predictions.get(key).map(Vec::as_slice).unwrap_or(&[]);
        let mut previous: Option<Truth> = None;
        for (timestamp, truth) in annotations {
            if *truth == Truth::Unknown {
                previous = None;
                continue;
            }
            if let Some(before) = previous.filter(|before| *before != *truth) {
                let (direction, latencies, target) = if before == Truth::Free {
                    (
                        &mut result.free_to_occupied,
                        &mut occupied_latencies,
                        ObservedState::Occupied,
                    )
                } else {
                    (
                        &mut result.occupied_to_free,
                        &mut free_latencies,
                        ObservedState::Free,
                    )
                };
                direction.transition_count += 1;
                let deadline = timestamp.saturating_add(timeout_ms);
                let detected = samples.iter().find(|point| {
                    point.timestamp_ms >= *timestamp
                        && point.timestamp_ms <= deadline
                        && point.stable_established
                        && point.stabilized == target
                });
                if let Some(point) = detected {
                    direction.detected_transition_count += 1;
                    latencies.push(point.timestamp_ms - timestamp);
                } else {
                    direction.missed_transition_count += 1;
                }
            }
            previous = Some(*truth);
        }
    }
    finish_latencies(&mut result.free_to_occupied, &mut occupied_latencies);
    finish_latencies(&mut result.occupied_to_free, &mut free_latencies);
    result.transition_count =
        result.free_to_occupied.transition_count + result.occupied_to_free.transition_count;
    result.detected_transition_count = result.free_to_occupied.detected_transition_count
        + result.occupied_to_free.detected_transition_count;
    result.missed_transition_count = result.free_to_occupied.missed_transition_count
        + result.occupied_to_free.missed_transition_count;
    result.extra_state_changes = result
        .stable_state_changes_count
        .saturating_sub(result.transition_count);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(truth: Option<Truth>, prediction: ObservedState) -> Point {
        Point { truth, prediction }
    }

    #[test]
    fn confusion_matrix_counts_all_predictions() {
        let m = classify([
            point(Some(Truth::Free), ObservedState::Free),
            point(Some(Truth::Free), ObservedState::Uncertain),
            point(Some(Truth::Occupied), ObservedState::Free),
            point(Some(Truth::Occupied), ObservedState::Occupied),
        ]);
        assert_eq!(m.confusion.pred_free_truth_free, 1);
        assert_eq!(m.confusion.pred_uncertain_truth_free, 1);
        assert_eq!(m.confusion.pred_free_truth_occupied, 1);
        assert_eq!(m.confusion.pred_occupied_truth_occupied, 1);
        assert_eq!(m.coverage, Some(0.75));
    }

    #[test]
    fn false_free_rate_is_computed_correctly() {
        let m = classify([
            point(Some(Truth::Occupied), ObservedState::Free),
            point(Some(Truth::Occupied), ObservedState::Occupied),
        ]);
        assert_eq!(m.false_free_count, 1);
        assert_eq!(m.false_free_rate, Some(0.5));
    }

    #[test]
    fn uncertain_predictions_reduce_coverage() {
        let m = classify([
            point(Some(Truth::Free), ObservedState::Free),
            point(Some(Truth::Free), ObservedState::Uncertain),
        ]);
        assert_eq!(m.coverage, Some(0.5));
    }

    #[test]
    fn unknown_ground_truth_is_excluded_from_classification_metrics() {
        let m = classify([
            point(Some(Truth::Unknown), ObservedState::Free),
            point(None, ObservedState::Occupied),
            point(Some(Truth::Free), ObservedState::Free),
        ]);
        assert_eq!(m.labelled_samples, 1);
        assert_eq!(m.excluded_unknown_count, 1);
        assert_eq!(m.unlabelled_prediction_count, 1);
        assert_eq!(m.strict_accuracy, Some(1.0));
    }

    fn evaluated(time: i64, state: ObservedState) -> EvaluatedPoint {
        EvaluatedPoint {
            video_id: "v".into(),
            camera_id: Uuid::nil(),
            spot_id: Uuid::nil(),
            timestamp_ms: time,
            truth: None,
            frame: state,
            stabilized: state,
            stable_established: true,
        }
    }

    #[test]
    fn free_to_occupied_latency_is_measured() {
        let truths = BTreeMap::from([(
            ("v".into(), Uuid::nil()),
            vec![(0, Truth::Free), (1000, Truth::Occupied)],
        )]);
        let m = transition_metrics(
            &truths,
            &[
                evaluated(0, ObservedState::Free),
                evaluated(1500, ObservedState::Occupied),
            ],
            1000,
        );
        assert_eq!(m.free_to_occupied.mean_latency_ms, Some(500.0));
    }

    #[test]
    fn transition_not_detected_before_timeout_is_missed() {
        let truths = BTreeMap::from([(
            ("v".into(), Uuid::nil()),
            vec![(0, Truth::Free), (1000, Truth::Occupied)],
        )]);
        let m = transition_metrics(
            &truths,
            &[
                evaluated(0, ObservedState::Free),
                evaluated(3000, ObservedState::Occupied),
            ],
            1000,
        );
        assert_eq!(m.missed_transition_count, 1);
    }

    #[test]
    fn extra_state_changes_detect_flapping() {
        let truths = BTreeMap::from([(
            ("v".into(), Uuid::nil()),
            vec![(0, Truth::Free), (1000, Truth::Occupied)],
        )]);
        let m = transition_metrics(
            &truths,
            &[
                evaluated(0, ObservedState::Free),
                evaluated(1000, ObservedState::Occupied),
                evaluated(1500, ObservedState::Free),
                evaluated(2000, ObservedState::Occupied),
            ],
            1000,
        );
        assert_eq!(m.stable_state_changes_count, 3);
        assert_eq!(m.extra_state_changes, 2);
    }

    #[test]
    fn unknown_gap_does_not_create_transition() {
        let truths = BTreeMap::from([(
            ("v".into(), Uuid::nil()),
            vec![
                (0, Truth::Free),
                (500, Truth::Unknown),
                (1000, Truth::Occupied),
            ],
        )]);
        let m = transition_metrics(&truths, &[evaluated(1000, ObservedState::Occupied)], 1000);
        assert_eq!(m.transition_count, 0);
    }

    #[test]
    fn transition_without_prediction_is_missed() {
        let truths = BTreeMap::from([(
            ("v".into(), Uuid::nil()),
            vec![(0, Truth::Free), (1000, Truth::Occupied)],
        )]);
        let m = transition_metrics(&truths, &[], 1000);
        assert_eq!(m.transition_count, 1);
        assert_eq!(m.missed_transition_count, 1);
    }
}
