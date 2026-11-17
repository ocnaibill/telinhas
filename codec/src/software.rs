//! Software H.264 through `openh264`: the fallback when there is no hardware
//! encoder. Slower, and it skips frames when the content is too hard for the
//! bitrate, so the interface warns about it (DEC-15).

use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Encoder as OpenH264Encoder, EncoderConfig, FrameRate, IntraFramePeriod,
    RateControlMode, UsageType,
};
use openh264::formats::{BgraSliceU8, YUVBuffer};

use crate::{
    EncodedPacket, Encoder, Error, Frame, Info, Options, Result, contains_idr, packed_rows,
};

pub struct Software {
    options: Options,
    encoder: OpenH264Encoder,
    scratch: Vec<u8>,
    force_key: bool,
}

fn open(options: &Options) -> Result<OpenH264Encoder> {
    let format = options.format;
    if !format.width.is_multiple_of(2) || !format.height.is_multiple_of(2) {
        return Err(Error::BadInput(
            "the software encoder needs even width and height".to_owned(),
        ));
    }
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(
            options.rate.max_kbps.saturating_mul(1000),
        ))
        .max_frame_rate(FrameRate::from_hz(format.fps as f32))
        .usage_type(UsageType::ScreenContentRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .skip_frames(true)
        .intra_frame_period(IntraFramePeriod::from_num_frames(options.keyframe_every));
    OpenH264Encoder::with_api_config(OpenH264API::from_source(), config)
        .map_err(|error| Error::Backend(error.to_string()))
}

impl Software {
    pub fn open(options: Options) -> Result<Self> {
        Ok(Self {
            encoder: open(&options)?,
            options,
            scratch: Vec::new(),
            force_key: false,
        })
    }
}

impl Encoder for Software {
    fn encode(&mut self, frame: &Frame<'_>) -> Result<Vec<EncodedPacket>> {
        let packed = packed_rows(frame, self.options.format, &mut self.scratch)?;
        let size = (frame.width as usize, frame.height as usize);
        let yuv = YUVBuffer::from_bgra8_source(BgraSliceU8::new(packed, size));
        if std::mem::take(&mut self.force_key) {
            self.encoder.force_intra_frame();
        }
        let stream = self
            .encoder
            .encode(&yuv)
            .map_err(|error| Error::Backend(error.to_string()))?;
        let data = stream.to_vec();
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let keyframe = contains_idr(&data);
        Ok(vec![EncodedPacket { data, keyframe }])
    }

    fn request_keyframe(&mut self) {
        self.force_key = true;
    }

    fn set_max_kbps(&mut self, kbps: u32) -> Result<()> {
        if kbps == self.options.rate.max_kbps {
            return Ok(());
        }
        self.options.rate.max_kbps = kbps;
        self.encoder = open(&self.options)?;
        // A fresh encoder starts with a keyframe on its own.
        Ok(())
    }

    fn info(&self) -> Info {
        Info {
            name: "openh264 (software)".to_owned(),
            hardware: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Format, Rate};

    fn options() -> Options {
        Options {
            format: Format {
                width: 320,
                height: 240,
                fps: 30,
            },
            rate: Rate {
                quality: 23,
                max_kbps: 1000,
            },
            keyframe_every: 0,
        }
    }

    /// A moving pattern, so the encoder has something to do.
    fn picture(index: usize) -> Vec<u8> {
        let (w, h) = (320usize, 240usize);
        let mut bgra = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let v = ((x + y + index * 4) % 256) as u8;
                let at = (y * w + x) * 4;
                bgra[at] = v;
                bgra[at + 1] = v / 2;
                bgra[at + 2] = 255 - v;
            }
        }
        bgra
    }

    fn encode(encoder: &mut Software, index: usize) -> Vec<EncodedPacket> {
        let data = picture(index);
        let frame = Frame {
            width: 320,
            height: 240,
            stride: 320 * 4,
            bgra: &data,
        };
        encoder.encode(&frame).unwrap()
    }

    #[test]
    fn first_frame_is_a_keyframe_and_the_rest_are_not() {
        let mut encoder = Software::open(options()).unwrap();
        let first = encode(&mut encoder, 0);
        assert_eq!(first.len(), 1);
        assert!(first[0].keyframe);
        for index in 1..6 {
            for packet in encode(&mut encoder, index) {
                assert!(!packet.keyframe, "frame {index} should be a P frame");
            }
        }
    }

    #[test]
    fn a_requested_keyframe_arrives_on_the_next_picture() {
        let mut encoder = Software::open(options()).unwrap();
        for index in 0..4 {
            encode(&mut encoder, index);
        }
        encoder.request_keyframe();
        let packets = encode(&mut encoder, 4);
        assert!(packets.iter().any(|p| p.keyframe));
    }

    #[test]
    fn changing_the_ceiling_keeps_the_encoder_working() {
        let mut encoder = Software::open(options()).unwrap();
        encode(&mut encoder, 0);
        encoder.set_max_kbps(300).unwrap();
        let after = encode(&mut encoder, 1);
        // A new encoder opens with a keyframe.
        assert!(after.iter().any(|p| p.keyframe));
        assert!(!encoder.info().hardware);
    }

    #[test]
    fn odd_sizes_are_refused() {
        let mut bad = options();
        bad.format.width = 321;
        assert!(Software::open(bad).is_err());
    }
}
