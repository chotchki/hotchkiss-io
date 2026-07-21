//! Responsive image variants (Phase CN): downscale a source image to a few
//! widths and AVIF-encode each, so the render can emit a `srcset` and the browser
//! pulls an appropriately-sized file instead of the full-resolution original
//! (PSI "improve image delivery" flagged ~434 KiB of oversize on the home page).
//!
//! Reuses the SAME in-process `image`-crate AVIF path as the video poster
//! (`poster.rs`) — proven on dev + the mini — so there's no new ffmpeg-build
//! feature dependency. Sync (decode + rav1e encode block) → call under
//! `spawn_blocking`.

use anyhow::{anyhow, bail, Context, Result};
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use crate::db::dao::media::EditParams;
use crate::media::probe::resolve_bin;

/// Target widths for the srcset ladder. A width >= the source is skipped (never
/// upscale); the original variant stays as the largest fallback. Capped at 960:
/// in-flow content images render at a 480px max-height, so 960 covers a 2× phone
/// and keeps the per-upload / backfill encode bounded (rav1e is not fast).
pub const RESPONSIVE_WIDTHS: [u32; 2] = [480, 960];

/// Cap for the EXTRA "full" rung minted when the source format isn't
/// browser-displayable (a HEIC, EB.9): that rung becomes the embed src + zoom
/// view, and rav1e at a true 12MP phone resolution is minutes-slow. The genuine
/// original stays stored for download; 1920 is 2× the ladder top.
pub const NON_WEB_FULL_WIDTH_CAP: u32 = 1920;

static FFMPEG_BIN: LazyLock<Option<String>> =
    LazyLock::new(|| resolve_bin("FFMPEG_BIN", "ffmpeg"));

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
pub fn responsive_avif_variants(path: &Path, edit: &EditParams) -> Result<ResizeResult> {
    let (img, web_native) = decode_source(path)?;
    let img = apply_edit(img, edit);
    let (source_width, source_height) = (img.width(), img.height());

    let edited = edit.rotate % 4 != 0 || edit.corners.is_some();
    let mut widths = target_widths(source_width);
    if !web_native || edited {
        // EB.9/ED: the source bytes can't stand in for the viewing image —
        // either the format doesn't render in most browsers (HEIC) or an EDIT
        // means the original's pixels are no longer the picture. Mint the
        // capped "full" rung so the ladder top (embed src + zoom) never
        // depends on the source.
        let full = source_width.min(NON_WEB_FULL_WIDTH_CAP);
        if !widths.contains(&full) {
            widths.push(full);
        }
    }

    let mut variants = Vec::new();
    for w in widths {
        // Aspect-preserving box; resize() fits WITHIN (w, h) so read the actual
        // result dims back for the srcset `Nw` descriptor.
        let resized = if w == source_width {
            img.clone()
        } else {
            let h =
                ((source_height as u64 * w as u64) / (source_width.max(1) as u64)).max(1) as u32;
            img.resize(w, h, FilterType::Lanczos3)
        };
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

/// Apply the item's edit params to the decoded source (ED): quarter-turn
/// rotation FIRST (the crop UI shows the rotated view, so corners are defined
/// in the rotated frame), then the 4-corner crop/perspective warp. The edit is
/// an input to DERIVATION — the stored original is never touched. A degenerate
/// quad falls back to the un-cropped image (warn, never a hard failure — the
/// endpoint validates, this is the belt-and-suspenders).
fn apply_edit(img: DynamicImage, edit: &EditParams) -> DynamicImage {
    let img = match edit.rotate % 4 {
        1 => img.rotate90(),
        2 => img.rotate180(),
        3 => img.rotate270(),
        _ => img,
    };
    let Some(corners) = edit.corners else {
        return img;
    };
    match warp_quad(&img, &corners) {
        Some(warped) => warped,
        None => {
            tracing::warn!("degenerate crop quad {corners:?} — serving un-cropped");
            img
        }
    }
}

/// Homography-warp the normalized corner quad (TL, TR, BR, BL) flat into a
/// rectangle (ED.4): an axis-aligned quad is a plain crop; a skewed one lays an
/// angled sheet of paper flat. Target dims = the average opposing edge lengths
/// in source pixels, so the output resolution matches what the quad actually
/// covers. `None` on a degenerate/non-invertible quad or a sub-8px target.
fn warp_quad(img: &DynamicImage, corners: &[[f64; 2]; 4]) -> Option<DynamicImage> {
    use imageproc::geometric_transformations::{warp_into, Interpolation, Projection};

    let (w, h) = (img.width() as f64, img.height() as f64);
    let px: Vec<(f32, f32)> = corners
        .iter()
        .map(|[x, y]| {
            (
                (x.clamp(0.0, 1.0) * w) as f32,
                (y.clamp(0.0, 1.0) * h) as f32,
            )
        })
        .collect();
    let dist = |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let target_w = ((dist(px[0], px[1]) + dist(px[3], px[2])) / 2.0).round().max(0.0) as u32;
    let target_h = ((dist(px[0], px[3]) + dist(px[1], px[2])) / 2.0).round().max(0.0) as u32;
    if target_w < 8 || target_h < 8 {
        return None;
    }

    let from = [px[0], px[1], px[2], px[3]];
    let to = [
        (0.0, 0.0),
        (target_w as f32, 0.0),
        (target_w as f32, target_h as f32),
        (0.0, target_h as f32),
    ];
    let projection = Projection::from_control_points(from, to)?;

    let src = img.to_rgba8();
    let mut out = image::RgbaImage::new(target_w, target_h);
    warp_into(
        &src,
        &projection,
        Interpolation::Bilinear,
        image::Rgba([0, 0, 0, 255]),
        &mut out,
    );
    Some(DynamicImage::ImageRgba8(out))
}

/// Decode the source pixels. The `image` crate covers every web-native format;
/// anything it can't read (HEIC/HEIF — the iPhone default) falls back to a
/// one-frame ffmpeg decode (a still is a 1-frame stream to ffmpeg; mirrors
/// poster.rs). The bool is "the SOURCE format is browser-displayable" — false
/// tells the caller to mint the capped full rung.
fn decode_source(path: &Path) -> Result<(image::DynamicImage, bool)> {
    match decode_native_oriented(path) {
        Ok(img) => Ok((img, true)),
        Err(native_err) => {
            let Some(bin) = FFMPEG_BIN.as_deref() else {
                bail!(
                    "decoding {}: {native_err} (and no ffmpeg for the HEIC fallback)",
                    path.display()
                );
            };
            let png = ffmpeg_first_frame_png(bin, path).with_context(|| {
                format!(
                    "decoding {} (image crate: {native_err}; ffmpeg fallback also failed)",
                    path.display()
                )
            })?;
            let img = image::load_from_memory(&png).context("decoding ffmpeg fallback PNG")?;
            Ok((img, false))
        }
    }
}

/// Native decode with the source's EXIF orientation APPLIED (EB.10): the image
/// crate's plain `decode()` returns RAW pixels — a phone JPEG stores them
/// unrotated + an Orientation tag, so every derived AVIF rung rendered sideways
/// (browsers honor the tag on the original; our re-encodes drop it). Read the
/// orientation off the decoder BEFORE consuming it, then bake it in.
fn decode_native_oriented(path: &Path) -> Result<DynamicImage> {
    let mut decoder = ImageReader::open(path)
        .with_context(|| format!("opening {} for resize", path.display()))?
        .with_guessed_format()?
        .into_decoder()
        .with_context(|| format!("decoding {}", path.display()))?;
    let orientation = decoder
        .orientation()
        .unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)
        .with_context(|| format!("decoding {}", path.display()))?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// One PNG frame on stdout. ffmpeg's default autorotate bakes in a HEIC's
/// irot/display-matrix rotation (an orientation-6 HEIC decodes width/height
/// swapped, upright — pinned by `rotated_heic_bakes_orientation_into_the_avif`).
/// NOT passed explicitly: `-autorotate` is a bare flag whose argument-less form
/// can't be pinned "on" without ffmpeg misparsing the next token as an output.
fn ffmpeg_first_frame_png(bin: &str, path: &Path) -> Result<Vec<u8>> {
    let out = Command::new(bin)
        .args(["-v", "error"])
        .arg("-i")
        .arg(path)
        .args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "-"])
        .output()
        .map_err(|e| anyhow!("failed to spawn ffmpeg ({bin}): {e}"))?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!(
            "ffmpeg decode failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
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
    fn corner_crop_warps_to_the_quad_rect() {
        // ED.4: an axis-aligned quad = a plain crop. Center 50% of a 400x200
        // gradient → a ~200x100 output whose LEFT edge shows the source's 25%
        // column color (red = x, so ~100).
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("crop.png");
        let img = image::RgbImage::from_fn(400, 200, |x, _| {
            image::Rgb([(x / 2).min(255) as u8, 10, 10])
        });
        image::DynamicImage::ImageRgb8(img).save(&src).unwrap();

        let edit = EditParams {
            rotate: 0,
            corners: Some([[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]]),
        };
        let out = responsive_avif_variants(&src, &edit).unwrap();
        assert_eq!(
            (out.source_width, out.source_height),
            (200, 100),
            "the derivation source is the CROPPED frame"
        );
        assert_eq!(out.variants.len(), 1, "edited => full rung minted");

        // A skewed (perspective) quad still produces a flat rect of the
        // averaged edge lengths — the angled-paper case.
        let edit = EditParams {
            rotate: 0,
            corners: Some([[0.3, 0.25], [0.75, 0.3], [0.7, 0.75], [0.25, 0.7]]),
        };
        let out = responsive_avif_variants(&src, &edit).unwrap();
        assert!(
            out.source_width >= 8 && out.source_height >= 8,
            "perspective quad warps to a usable rect ({}x{})",
            out.source_width,
            out.source_height
        );
    }

    #[test]
    fn rotate_edit_is_applied_and_mints_the_full_rung() {
        // ED.3: rotate=1 (90 CW) — the decoded source comes out PORTRAIT and,
        // because the item is EDITED, a full-size rung is minted even for a
        // web-native source (the original can no longer stand in as the src).
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("rot-edit.png");
        let img = image::RgbImage::from_fn(300, 200, |x, _| {
            image::Rgb([(x % 256) as u8, 32, 200])
        });
        image::DynamicImage::ImageRgb8(img).save(&src).unwrap();

        let edit = EditParams {
            rotate: 1,
            corners: None,
        };
        let out = responsive_avif_variants(&src, &edit).unwrap();
        assert_eq!((out.source_width, out.source_height), (200, 300));
        assert_eq!(out.variants.len(), 1, "edited => the full rung exists");
        assert_eq!(
            (out.variants[0].width, out.variants[0].height),
            (200, 300),
            "the rung is the rotated full frame"
        );
    }

    #[test]
    fn exif_orientation_is_baked_into_the_rungs() {
        // A phone JPEG stores pixels UNROTATED + an EXIF Orientation tag (EB.10
        // — the sideways-capture bug). Build one hermetically: encode 300x200,
        // splice a minimal APP1 with Orientation=6 (rotate 90 CW) after the SOI.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("rot.jpg");
        let img = image::RgbImage::from_fn(300, 200, |x, _| {
            image::Rgb([(x % 256) as u8, 64, 128])
        });
        let mut jpeg = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut jpeg), ImageFormat::Jpeg)
            .unwrap();
        assert_eq!(&jpeg[..2], b"\xff\xd8");
        let tiff: &[u8] = b"II*\x00\x08\x00\x00\x00\
\x01\x00\
\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\
\x00\x00\x00\x00";
        let payload: Vec<u8> = [b"Exif\x00\x00".as_slice(), tiff].concat();
        let mut app1 = vec![0xff, 0xe1];
        app1.extend(((payload.len() + 2) as u16).to_be_bytes());
        app1.extend(&payload);
        let spliced: Vec<u8> = [&jpeg[..2], &app1, &jpeg[2..]].concat();
        std::fs::write(&src, spliced).unwrap();

        let out = responsive_avif_variants(&src, &EditParams::default()).unwrap();
        // Orientation 6 applied → the decoded source is PORTRAIT 200x300.
        assert_eq!(
            (out.source_width, out.source_height),
            (200, 300),
            "EXIF orientation must be baked in, not dropped"
        );
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

        let out = responsive_avif_variants(&src, &EditParams::default()).unwrap();
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
