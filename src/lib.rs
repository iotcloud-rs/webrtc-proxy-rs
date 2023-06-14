use std::time::Duration;

use retina::codec::VideoFrame;
use webrtc::media::Sample;

pub mod signaling;
pub mod trace;

pub fn h264_to_sample(frame: VideoFrame) -> Sample {
    let mut data = frame.into_data();
    let mut i = 0;
    while i < data.len() - 3 {
        // Replace each NAL's length with the Annex B start code b"\x00\x00\x00\x01".
        let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        data[i] = 0;
        data[i + 1] = 0;
        data[i + 2] = 0;
        data[i + 3] = 1;
        i += 4 + len;
        if i > data.len() {
            todo!("partial NAL body");
        }
    }
    if i < data.len() {
        todo!("partial NAL length");
    }
    Sample {
        data: data.into(),
        duration: Duration::from_secs(1),
        ..Default::default()
    }
}
