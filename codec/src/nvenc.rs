//! Hardware H.264 on NVIDIA GPUs (NVENC) through the FFmpeg libraries.
//!
//! Settings follow the measurements in `docs/PLANO-M3.md`: low-latency preset,
//! no B-frames, constant quality with a bitrate ceiling, keyframes on demand.

use ffmpeg_the_third as ffmpeg;

use ffmpeg::format::Pixel;
use ffmpeg::{Dictionary, Packet, codec, encoder, frame, picture};

use crate::{EncodedPacket, Encoder, Error, Frame, Info, Options, Result, packed_rows};

const ENCODER_NAME: &str = "h264_nvenc";
/// With keyframes on demand, still send one this often as a safety net.
const SAFETY_KEYFRAME_SECONDS: u32 = 60;

pub struct Nvenc {
    options: Options,
    encoder: encoder::video::Encoder,
    picture: frame::Video,
    scratch: Vec<u8>,
    index: i64,
    force_key: bool,
}

fn backend_error(error: ffmpeg::Error) -> Error {
    Error::Backend(error.to_string())
}

fn open(options: &Options) -> Result<encoder::video::Encoder> {
    ffmpeg::init().map_err(backend_error)?;
    let format = options.format;
    let codec = encoder::find_by_name(ENCODER_NAME)
        .ok_or_else(|| Error::Unavailable(format!("{ENCODER_NAME} is not in this FFmpeg build")))?;

    let mut video = codec::Context::new_with_codec(codec)
        .encoder()
        .video()
        .map_err(backend_error)?;
    video.set_width(format.width);
    video.set_height(format.height);
    video.set_format(Pixel::BGRA);
    video.set_time_base((1, format.fps as i32));
    video.set_frame_rate(Some((format.fps as i32, 1)));
    video.set_max_b_frames(0);
    let gop = if options.keyframe_every == 0 {
        format.fps * SAFETY_KEYFRAME_SECONDS
    } else {
        options.keyframe_every
    };
    video.set_gop(gop);

    let max_bps = u64::from(options.rate.max_kbps) * 1000;
    let mut settings = Dictionary::new();
    settings.set("preset", "p1");
    settings.set("tune", "ll");
    settings.set("rc", "vbr");
    settings.set("cq", options.rate.quality.to_string());
    settings.set("b", "0");
    settings.set("maxrate", max_bps.to_string());
    settings.set("bufsize", (max_bps / 2).to_string());
    settings.set("zerolatency", "1");
    settings.set("rc-lookahead", "0");
    // Without this a requested I-frame is not an IDR, and a new viewer could
    // not start decoding from it.
    settings.set("forced-idr", "1");
    video.open_with(settings).map_err(backend_error)
}

impl Nvenc {
    pub fn open(options: Options) -> Result<Self> {
        let encoder = open(&options)?;
        let picture = frame::Video::new(Pixel::BGRA, options.format.width, options.format.height);
        Ok(Self {
            options,
            encoder,
            picture,
            scratch: Vec::new(),
            index: 0,
            force_key: false,
        })
    }

    fn drain(&mut self) -> Result<Vec<EncodedPacket>> {
        let mut out = Vec::new();
        loop {
            let mut packet = Packet::empty();
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => out.push(EncodedPacket {
                    data: packet.data().map(<[u8]>::to_vec).unwrap_or_default(),
                    keyframe: packet.is_key(),
                }),
                Err(ffmpeg::Error::Other { errno }) if errno == libc::EAGAIN => break,
                Err(ffmpeg::Error::Eof) => break,
                Err(error) => return Err(backend_error(error)),
            }
        }
        Ok(out)
    }
}

impl Encoder for Nvenc {
    fn encode(&mut self, frame: &Frame<'_>) -> Result<Vec<EncodedPacket>> {
        let width = self.options.format.width as usize;
        let height = self.options.format.height as usize;
        let packed = packed_rows(frame, self.options.format, &mut self.scratch)?;

        let stride = self.picture.stride(0);
        let target = self.picture.data_mut(0);
        for row in 0..height {
            let source = &packed[row * width * 4..][..width * 4];
            target[row * stride..][..width * 4].copy_from_slice(source);
        }
        self.picture.set_pts(Some(self.index));
        self.index += 1;
        self.picture
            .set_kind(if std::mem::take(&mut self.force_key) {
                picture::Type::I
            } else {
                picture::Type::None
            });

        self.encoder
            .send_frame(&self.picture)
            .map_err(backend_error)?;
        self.drain()
    }

    fn request_keyframe(&mut self) {
        self.force_key = true;
    }

    fn set_max_kbps(&mut self, kbps: u32) -> Result<()> {
        if kbps == self.options.rate.max_kbps {
            return Ok(());
        }
        self.options.rate.max_kbps = kbps;
        // NVENC reconfigures a running encoder: no reopen, no dropped frames
        // (measured: 0 ms against 449 ms and about 28 lost frames when
        // reopening). It emits one keyframe and the bitrate settles over a few
        // seconds.
        self.encoder.set_max_bit_rate(kbps as usize * 1000);
        Ok(())
    }

    fn info(&self) -> Info {
        Info {
            name: format!("{ENCODER_NAME} (FFmpeg)"),
            hardware: true,
        }
    }
}
