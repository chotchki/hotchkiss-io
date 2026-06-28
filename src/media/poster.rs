//! Auto-poster (Phase BZ): grab a frame from a video with ffmpeg and re-encode
//! it small as AVIF — used both as the `<video poster>` and as the library
//! thumbnail. Stored as a normal image-mime `media_variant` of the video, so a
//! manually-added poster (an image dropped via "+ add encode") is the same
//! thing and overrides it.

use anyhow::{anyhow, bail, Result};
use image::ImageFormat;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use crate::media::probe::resolve_bin;

/// Poster width cap (height scales). 640px covers a retina card thumbnail.
const POSTER_MAX_WIDTH: u32 = 640;

static FFMPEG_BIN: LazyLock<Option<String>> =
    LazyLock::new(|| resolve_bin("FFMPEG_BIN", "ffmpeg"));

/// Grab a representative frame (~1s in, falling back to the first frame for very
/// short clips) and re-encode it small as AVIF. Returns the AVIF bytes. Sync
/// (ffmpeg subprocess + AVIF encode both block) — call under `spawn_blocking`.
pub fn generate_poster(video_path: &Path) -> Result<Vec<u8>> {
    let bin = FFMPEG_BIN.as_deref().ok_or_else(|| {
        anyhow!("ffmpeg not found — `brew install ffmpeg` (looked at $FFMPEG_BIN, brew, PATH)")
    })?;

    let png = grab_frame(bin, video_path, Some("1"))
        .or_else(|_| grab_frame(bin, video_path, None))?;
    encode_avif(&png)
}

/// One PNG frame on stdout. `seek` is an optional `-ss` start time.
fn grab_frame(bin: &str, video_path: &Path, seek: Option<&str>) -> Result<Vec<u8>> {
    let mut cmd = Command::new(bin);
    cmd.args(["-v", "error"]);
    if let Some(s) = seek {
        cmd.args(["-ss", s]);
    }
    cmd.arg("-i").arg(video_path).args([
        "-frames:v",
        "1",
        "-f",
        "image2pipe",
        "-vcodec",
        "png",
        "-",
    ]);
    let out = cmd
        .output()
        .map_err(|e| anyhow!("failed to spawn ffmpeg ({bin}): {e}"))?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!(
            "ffmpeg frame grab failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

fn encode_avif(png: &[u8]) -> Result<Vec<u8>> {
    let img = image::ImageReader::new(Cursor::new(png))
        .with_guessed_format()?
        .decode()?;
    let img = if img.width() > POSTER_MAX_WIDTH {
        let h = (img.height() as u64 * POSTER_MAX_WIDTH as u64 / img.width().max(1) as u64).max(1)
            as u32;
        img.resize(POSTER_MAX_WIDTH, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Avif)?;
    Ok(buf)
}
