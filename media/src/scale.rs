//! Shrinking a BGRA picture, so a broadcast can be sent smaller than the
//! screen it comes from. Never the other way: the limit of a broadcast is what
//! its broadcaster chose.

/// Averages `src` (`sw` x `sh`) down into `dst` (`dw` x `dh`), as tightly
/// packed BGRA. Every source pixel falls into exactly one destination pixel,
/// so it costs one pass over the source.
pub(crate) fn downscale(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize, dst: &mut Vec<u8>) {
    debug_assert!(dw > 0 && dh > 0 && dw <= sw && dh <= sh);
    debug_assert_eq!(src.len(), sw * sh * 4);
    // Which destination column each source column feeds, and how many do.
    let xmap: Vec<usize> = (0..sw).map(|x| x * dw / sw).collect();
    let mut xcount = vec![0u32; dw];
    for &dx in &xmap {
        xcount[dx] += 1;
    }
    dst.clear();
    dst.reserve(dw * dh * 4);
    let mut sums = vec![0u32; dw * 4];
    let mut row = 0usize;
    for dy in 0..dh {
        let end = ((dy + 1) * sh).div_ceil(dh).min(sh);
        let start = row;
        sums.fill(0);
        while row < end {
            let line = &src[row * sw * 4..][..sw * 4];
            for (x, px) in line.as_chunks::<4>().0.iter().enumerate() {
                let at = xmap[x] * 4;
                sums[at] += u32::from(px[0]);
                sums[at + 1] += u32::from(px[1]);
                sums[at + 2] += u32::from(px[2]);
                sums[at + 3] += u32::from(px[3]);
            }
            row += 1;
        }
        let rows = (row - start).max(1) as u32;
        for (dx, count) in xcount.iter().enumerate() {
            let n = (count * rows).max(1);
            for c in 0..4 {
                dst.push((sums[dx * 4 + c] / n) as u8);
            }
        }
    }
}

/// The size to send a `width` x `height` picture at when the broadcaster
/// wants at most `max_height` lines: the same proportions, even sides, and
/// never larger than the picture.
pub(crate) fn target_size(width: u32, height: u32, max_height: Option<u32>) -> (u32, u32) {
    match max_height {
        Some(limit) if limit >= 2 && limit < height => {
            let h = limit & !1;
            let w = ((u64::from(width) * u64::from(h) / u64::from(height.max(1))) as u32) & !1;
            (w.max(2), h)
        }
        _ => (width, height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_picture_stays_flat() {
        let src = vec![200u8; 8 * 6 * 4];
        let mut dst = Vec::new();
        downscale(&src, 8, 6, 4, 3, &mut dst);
        assert_eq!(dst.len(), 4 * 3 * 4);
        assert!(dst.iter().all(|&v| v == 200));
    }

    #[test]
    fn blocks_are_averaged() {
        // 2x2 into 1x1: the mean of four pixels.
        let mut src = Vec::new();
        for v in [0u8, 100, 200, 100] {
            src.extend_from_slice(&[v, v, v, 255]);
        }
        let mut dst = Vec::new();
        downscale(&src, 2, 2, 1, 1, &mut dst);
        assert_eq!(dst, vec![100, 100, 100, 255]);
    }

    #[test]
    fn odd_ratios_cover_the_whole_picture() {
        let src = vec![7u8; 1920 * 1080 * 4];
        let mut dst = Vec::new();
        downscale(&src, 1920, 1080, 1280, 720, &mut dst);
        assert_eq!(dst.len(), 1280 * 720 * 4);
        assert!(dst.iter().all(|&v| v == 7));
    }

    #[test]
    fn the_size_never_grows() {
        assert_eq!(target_size(1920, 1080, Some(720)), (1280, 720));
        assert_eq!(target_size(1280, 720, Some(1080)), (1280, 720));
        assert_eq!(target_size(1920, 1080, None), (1920, 1080));
        assert_eq!(target_size(2560, 1440, Some(1080)), (1920, 1080));
        assert_eq!(target_size(1000, 563, Some(360)).1, 360);
    }
}
