use std::{error::Error, fmt, path::Path};

use chrono::Utc;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use gstreamer_video::{VideoFormat, VideoInfo};
use parking_perception::Frame;
const FRAME_WAIT_SECONDS: u64 = 5;

pub trait FrameSource {
    fn next_frame(&mut self) -> Result<Option<Frame>, FrameSourceError>;
    fn close(&mut self) -> Result<(), FrameSourceError>;
}

pub struct GstreamerFileSource {
    pipeline: gst::Pipeline,
    sink: AppSink,
    closed: bool,
}

impl GstreamerFileSource {
    pub fn open(path: &Path) -> Result<Self, FrameSourceError> {
        gst::init().map_err(FrameSourceError::from_error)?;
        let pipeline = gst::Pipeline::new();
        let source = gst::ElementFactory::make("filesrc")
            .property("location", path)
            .build()
            .map_err(FrameSourceError::from_error)?;
        let decoder = gst::ElementFactory::make("decodebin")
            .build()
            .map_err(FrameSourceError::from_error)?;
        let queue = gst::ElementFactory::make("queue")
            .build()
            .map_err(FrameSourceError::from_error)?;
        let converter = gst::ElementFactory::make("videoconvert")
            .build()
            .map_err(FrameSourceError::from_error)?;
        let sink = rgb_appsink();

        pipeline
            .add_many([&source, &decoder, &queue, &converter, sink.upcast_ref()])
            .map_err(FrameSourceError::from_error)?;
        source
            .link(&decoder)
            .map_err(FrameSourceError::from_error)?;
        gst::Element::link_many([&queue, &converter, sink.upcast_ref()])
            .map_err(FrameSourceError::from_error)?;

        let queue_sink = queue
            .static_pad("sink")
            .ok_or_else(|| FrameSourceError::new("queue has no sink pad"))?;
        decoder.connect_pad_added(move |_decoder, source_pad| {
            if queue_sink.is_linked() {
                return;
            }
            let is_video = source_pad
                .current_caps()
                .and_then(|caps| {
                    caps.structure(0)
                        .map(|structure| structure.name().starts_with("video/"))
                })
                .unwrap_or(false);
            if is_video {
                let _ = source_pad.link(&queue_sink);
            }
        });

        pipeline
            .set_state(gst::State::Playing)
            .map_err(FrameSourceError::from_error)?;
        Ok(Self {
            pipeline,
            sink,
            closed: false,
        })
    }

    /// Synthetic source for offline pipeline validation; requires videotestsrc.
    pub fn test_pattern(width: u32, height: u32) -> Result<Self, FrameSourceError> {
        gst::init().map_err(FrameSourceError::from_error)?;
        let pipeline = gst::Pipeline::new();
        let source = gst::ElementFactory::make("videotestsrc")
            .property("num-buffers", 1_i32)
            .build()
            .map_err(FrameSourceError::from_error)?;
        let converter = gst::ElementFactory::make("videoconvert")
            .build()
            .map_err(FrameSourceError::from_error)?;
        let sink = rgb_appsink_with_dimensions(Some((width, height)));
        pipeline
            .add_many([&source, &converter, sink.upcast_ref()])
            .map_err(FrameSourceError::from_error)?;
        gst::Element::link_many([&source, &converter, sink.upcast_ref()])
            .map_err(FrameSourceError::from_error)?;
        pipeline
            .set_state(gst::State::Playing)
            .map_err(FrameSourceError::from_error)?;
        Ok(Self {
            pipeline,
            sink,
            closed: false,
        })
    }

    fn sample_to_frame(sample: &gst::Sample) -> Result<Frame, FrameSourceError> {
        let caps = sample
            .caps()
            .ok_or_else(|| FrameSourceError::new("GStreamer sample has no caps"))?;
        let info = VideoInfo::from_caps(caps).map_err(FrameSourceError::from_error)?;
        if info.format() != VideoFormat::Rgb {
            return Err(FrameSourceError::new(format!(
                "expected RGB frame, got {}",
                info.name()
            )));
        }
        let buffer = sample
            .buffer()
            .ok_or_else(|| FrameSourceError::new("GStreamer sample has no buffer"))?;
        let map = buffer
            .map_readable()
            .map_err(FrameSourceError::from_error)?;
        let row_bytes = info.width() as usize * 3;
        let stride = usize::try_from(info.stride()[0])
            .map_err(|_| FrameSourceError::new("negative RGB row stride"))?;
        let mut pixels = Vec::with_capacity(row_bytes * info.height() as usize);
        for row in 0..info.height() as usize {
            let start = row * stride;
            let end = start + row_bytes;
            let bytes = map.as_slice().get(start..end).ok_or_else(|| {
                FrameSourceError::new("RGB buffer is shorter than negotiated caps")
            })?;
            pixels.extend_from_slice(bytes);
        }
        let mut frame = Frame::rgb(info.width(), info.height(), pixels, Utc::now());
        frame.video_timestamp_ms = buffer.pts().map(|pts| pts.mseconds());
        Ok(frame)
    }
}

fn rgb_appsink() -> AppSink {
    rgb_appsink_with_dimensions(None)
}

fn rgb_appsink_with_dimensions(dimensions: Option<(u32, u32)>) -> AppSink {
    let mut caps = gst::Caps::builder("video/x-raw").field("format", "RGB");
    if let Some((width, height)) = dimensions {
        caps = caps
            .field("width", width as i32)
            .field("height", height as i32);
    }
    AppSink::builder()
        .caps(&caps.build())
        .sync(true)
        .max_buffers(1)
        .build()
}

impl FrameSource for GstreamerFileSource {
    fn next_frame(&mut self) -> Result<Option<Frame>, FrameSourceError> {
        if self.closed {
            return Ok(None);
        }
        if let Some(sample) = self
            .sink
            .try_pull_sample(gst::ClockTime::from_seconds(FRAME_WAIT_SECONDS))
        {
            return Self::sample_to_frame(&sample).map(Some);
        }
        if let Some(bus) = self.pipeline.bus()
            && let Some(message) = bus.pop_filtered(&[gst::MessageType::Error])
            && let gst::MessageView::Error(error) = message.view()
        {
            return Err(FrameSourceError::new(format!(
                "GStreamer video error: {}",
                error.error()
            )));
        }
        if self.sink.is_eos() {
            Ok(None)
        } else {
            Err(FrameSourceError::new(
                "video source produced no frame for 5 seconds",
            ))
        }
    }

    fn close(&mut self) -> Result<(), FrameSourceError> {
        if !self.closed {
            self.pipeline
                .set_state(gst::State::Null)
                .map_err(FrameSourceError::from_error)?;
            self.closed = true;
        }
        Ok(())
    }
}

impl Drop for GstreamerFileSource {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameSourceError(String);

impl FrameSourceError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn from_error(error: impl Error) -> Self {
        Self(error.to_string())
    }
}

impl fmt::Display for FrameSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for FrameSourceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires system GStreamer and the videotestsrc/videoconvert plugins"]
    fn gstreamer_pipeline_produces_rgb_frames() {
        let mut source = GstreamerFileSource::test_pattern(64, 48).unwrap();
        let frame = source.next_frame().unwrap().unwrap();
        assert_eq!((frame.width, frame.height), (64, 48));
        assert_eq!(frame.pixels.len(), 64 * 48 * 3);
        assert!(frame.video_timestamp_ms.is_some());
        assert!(source.next_frame().unwrap().is_none());
    }
}
