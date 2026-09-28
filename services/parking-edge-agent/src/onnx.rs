use std::path::{Path, PathBuf};

use ort::{
    session::Session,
    value::{Tensor, TensorElementType, ValueType},
};
use parking_perception::{BoundingBox, Detection, DetectionError, Detector, Frame};

pub struct OnnxDetector {
    session: Session,
    input_width: u32,
    input_height: u32,
}

impl OnnxDetector {
    pub fn load(path: &Path, input_width: u32, input_height: u32) -> Result<Self, DetectionError> {
        if input_width == 0 || input_height == 0 {
            return Err(DetectionError::new(
                "model input dimensions must be positive",
            ));
        }
        let dylib_path = std::env::var_os("ORT_DYLIB_PATH")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                format!(
                    "{}onnxruntime{}",
                    std::env::consts::DLL_PREFIX,
                    std::env::consts::DLL_SUFFIX
                )
                .into()
            });
        let _ = ort::init_from(&dylib_path)
            .map_err(|error| {
                DetectionError::new(format!(
                    "cannot load ONNX Runtime dynamic library; set ORT_DYLIB_PATH: {error}"
                ))
            })?
            .commit();
        let session = Session::builder()
            .and_then(|mut builder| builder.commit_from_file(path))
            .map_err(|error| DetectionError::new(format!("cannot load ONNX model: {error}")))?;
        validate_model_contract(&session, input_width, input_height)?;
        Ok(Self {
            session,
            input_width,
            input_height,
        })
    }
}

fn validate_model_contract(
    session: &Session,
    input_width: u32,
    input_height: u32,
) -> Result<(), DetectionError> {
    let expected = [1, 3, i64::from(input_height), i64::from(input_width)];
    let input_valid = session.inputs().len() == 1
        && matches!(session.inputs()[0].dtype(), ValueType::Tensor { ty: TensorElementType::Float32, shape, .. } if shape.iter().copied().eq(expected));
    let output_valid = session.outputs().len() == 1
        && matches!(session.outputs()[0].dtype(), ValueType::Tensor { ty: TensorElementType::Float32, shape, .. } if shape.len() == 2 && shape[1] == 6 && shape[0] >= -1);
    if !input_valid || !output_valid {
        return Err(DetectionError::new(format!(
            "model does not implement ParkingDetectorV1: expected one float32 input [1,3,{input_height},{input_width}] and one float32 output [N,6]"
        )));
    }
    Ok(())
}

impl Detector for OnnxDetector {
    fn detect(&mut self, frame: &Frame) -> Result<Vec<Detection>, DetectionError> {
        let input = preprocess_stretch_rgb_chw(frame, self.input_width, self.input_height)?;
        let tensor = Tensor::from_array((
            [
                1_usize,
                3,
                self.input_height as usize,
                self.input_width as usize,
            ],
            input.into_boxed_slice(),
        ))
        .map_err(|error| DetectionError::new(format!("cannot create input tensor: {error}")))?;
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .map_err(|error| DetectionError::new(format!("ONNX inference failed: {error}")))?;
        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|error| DetectionError::new(format!("invalid model output type: {error}")))?;
        parse_parking_detector_v1(shape.as_ref(), data)
    }
}

pub fn preprocess_stretch_rgb_chw(
    frame: &Frame,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<f32>, DetectionError> {
    if frame.width == 0 || frame.height == 0 || target_width == 0 || target_height == 0 {
        return Err(DetectionError::new(
            "frame and target dimensions must be positive",
        ));
    }
    let expected = frame.width as usize * frame.height as usize * 3;
    if frame.pixels.len() != expected {
        return Err(DetectionError::new(format!(
            "RGB frame has {} bytes, expected {expected}",
            frame.pixels.len()
        )));
    }
    let plane = target_width as usize * target_height as usize;
    let mut output = vec![0.0_f32; plane * 3];
    for target_y in 0..target_height {
        let source_y =
            (u64::from(target_y) * u64::from(frame.height) / u64::from(target_height)) as u32;
        for target_x in 0..target_width {
            let source_x =
                (u64::from(target_x) * u64::from(frame.width) / u64::from(target_width)) as u32;
            let source = ((source_y * frame.width + source_x) * 3) as usize;
            let target = (target_y * target_width + target_x) as usize;
            for channel in 0..3 {
                output[channel * plane + target] =
                    f32::from(frame.pixels[source + channel]) / 255.0;
            }
        }
    }
    Ok(output)
}

pub fn parse_parking_detector_v1(
    shape: &[i64],
    data: &[f32],
) -> Result<Vec<Detection>, DetectionError> {
    if shape.len() != 2 || shape[1] != 6 || shape[0] < 0 || shape[0] as usize * 6 != data.len() {
        return Err(DetectionError::new(format!(
            "ParkingDetectorV1 output must have shape [N, 6], got {shape:?}"
        )));
    }
    let mut detections = Vec::with_capacity(shape[0] as usize);
    for row in data.chunks_exact(6) {
        let confidence = row[4];
        let class = row[5];
        if !confidence.is_finite()
            || !(0.0..=1.0).contains(&confidence)
            || !class.is_finite()
            || class < 0.0
            || f64::from(class) >= 4_294_967_296.0
            || class.fract() != 0.0
        {
            return Err(DetectionError::new(
                "invalid confidence or class ID in model output",
            ));
        }
        let bbox = BoundingBox::new(
            f64::from(row[0]),
            f64::from(row[1]),
            f64::from(row[2]),
            f64::from(row[3]),
        )
        .map_err(|error| DetectionError::new(format!("invalid bbox in model output: {error}")))?;
        detections.push(Detection {
            class_id: class as u32,
            confidence,
            bbox,
        });
    }
    Ok(detections)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;

    use super::*;

    #[test]
    fn preprocessing_resizes_normalizes_and_converts_hwc_to_chw() {
        let frame = Frame {
            width: 2,
            height: 1,
            pixels: Arc::from([255_u8, 0, 0, 0, 128, 255]),
            captured_at: Utc::now(),
            video_timestamp_ms: None,
        };
        let output = preprocess_stretch_rgb_chw(&frame, 2, 1).unwrap();
        assert_eq!(output, vec![1.0, 0.0, 0.0, 128.0 / 255.0, 0.0, 1.0]);
    }

    #[test]
    fn invalid_model_row_fails_frame_without_panicking() {
        let output = [
            0.1,
            0.2,
            0.8,
            0.9,
            0.75,
            2.0, // valid
            f32::NAN,
            0.0,
            1.0,
            1.0,
            0.9,
            2.0, // invalid bbox
        ];
        assert!(parse_parking_detector_v1(&[2, 6], &output).is_err());
    }

    #[test]
    fn output_contract_requires_n_by_six() {
        assert!(parse_parking_detector_v1(&[1, 5], &[0.0; 5]).is_err());
    }

    #[test]
    fn invalid_confidence_and_class_fail_frame() {
        let output = [
            0.1,
            0.1,
            0.9,
            0.9,
            1.1,
            2.0,
            0.1,
            0.1,
            0.9,
            0.9,
            0.8,
            4_294_967_296.0,
        ];
        assert!(parse_parking_detector_v1(&[2, 6], &output).is_err());
    }
}
