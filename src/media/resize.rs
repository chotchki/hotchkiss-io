//! Responsive image variants (Phase CN): downscale a source image to a few
//! widths and AVIF-encode each, so the render can emit a `srcset` and the browser
//! pulls an appropriately-sized file instead of the full-resolution original
//! (PSI "improve image delivery" flagged ~434 KiB of oversize on the home page).
//!
//! Reuses the SAME in-process `image`-crate AVIF path as the video poster
//! (`poster.rs`) — proven on dev + the mini — so there's no new ffmpeg-build
//! feature dependency. Sync (decode + rav1e encode block) → call under
//! `spawn_blocking`.

use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{ImageFormat, ImageReader};
use std::io::Cursor;
use std::path::Path;

/// Target widths for the srcset ladder. A width >= the source is skipped (never
/// upscale); the original variant stays as the largest fallback. Capped at 960:
/// in-flow content images render at a 480px max-height, so 960 covers a 2× phone
/// and keeps the per-upload / backfill encode bounded (rav1e is not fast).
pub const RESPONSIVE_WIDTHS: [u32; 2] = [480, 960];

/// One generated variant: its actual pixel dimensions + the AVIF bytes.
pub struct ResizedImage {
    pub width: u32,
    pub height: u32,
    pub avif: Vec<u8>,
}

/// Resizing a source image: its TRUE dimensions (read from the decoded pixels,
/// NOT a possibly-NULL DB column) + the downscaled AVIF variants.
pub struct ResizeResult {
    pub source_width: u32,
    pub source_height: u32,
    pub variants: Vec<ResizedImage>,
}

/// For a stored source image at `path`, produce a downscaled AVIF for each
/// [`RESPONSIVE_WIDTHS`] strictly smaller than the source. Decodes once and reads
/// the source dimensions FROM THE PIXELS — so it works for a media item whose
/// `width` column is NULL (e.g. an attachment-migrated cover), which the old
/// width-taking signature silently skipped. `variants` is empty when the source
/// is already small (no width below it) — the original then serves alone.
pub fn responsive_avif_variants(path: &Path) -> Result<ResizeResult> {
    let img = ImageReader::open(path)
        .with_context(|| format!("opening {} for resize", path.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("decoding {}", path.display()))?;
    let (source_width, source_height) = (img.width(), img.height());

    let mut variants = Vec::new();
    for w in target_widths(source_width) {
        // Aspect-preserving box; resize() fits WITHIN (w, h) so read the actual
        // result dims back for the srcset `Nw` descriptor.
        let h = ((source_height as u64 * w as u64) / (source_width.max(1) as u64)).max(1) as u32;
        let resized = img.resize(w, h, FilterType::Lanczos3);
        let mut avif = Vec::new();
        resized
            .write_to(&mut Cursor::new(&mut avif), ImageFormat::Avif)
            .with_context(|| format!("AVIF-encoding width {w}"))?;
        variants.push(ResizedImage {
            width: resized.width(),
            height: resized.height(),
            avif,
        });
    }
    Ok(ResizeResult {
        source_width,
        source_height,
        variants,
    })
}

/// The [`RESPONSIVE_WIDTHS`] strictly smaller than the source (never upscale).
fn target_widths(source_width: u32) -> Vec<u32> {
    RESPONSIVE_WIDTHS
        .iter()
        .copied()
        .filter(|&w| w < source_width)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_widths_skip_at_or_above_source() {
        assert_eq!(target_widths(2000), vec![480, 960]);
        assert_eq!(target_widths(700), vec![480]); // 960 >= 700 → skipped
        assert_eq!(target_widths(480), Vec::<u32>::new()); // not strictly less
        assert_eq!(target_widths(100), Vec::<u32>::new());
    }

    #[test]
    fn resizes_a_source_image_to_an_avif_downscale() {
        // 700px source → exactly one downscale at 480 (960 is skipped), so the
        // test pays for a single rav1e encode.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.png");
        let img = image::RgbImage::from_fn(700, 400, |x, _| {
            image::Rgb([(x % 256) as u8, 128, 64])
        });
        image::DynamicImage::ImageRgb8(img).save(&src).unwrap();

        let out = responsive_avif_variants(&src).unwrap();
        // Source dims come from the decoded pixels (NOT a DB column).
        assert_eq!((out.source_width, out.source_height), (700, 400));
        assert_eq!(out.variants.len(), 1);
        assert_eq!(out.variants[0].width, 480);
        // Aspect preserved: 400 * 480 / 700 ≈ 274.
        assert!(
            (273..=275).contains(&out.variants[0].height),
            "height {}",
            out.variants[0].height
        );
        // AVIF container: an `ftyp` box carrying the `avif` brand near the start.
        assert!(
            out.variants[0].avif.windows(4).any(|w| w == b"avif"),
            "output is not AVIF"
        );
    }
}
