//! ffprobe-based media probing for ingest (Phase BZ). Given a stored file, derive
//! the TYPED kind + the `<source type codecs>` info from the actual bytes, never
//! trusting the upload's filename. STL is detected by extension (ffprobe doesn't
//! speak 3D); everything else goes through ffprobe. The image-vs-video call hinges
//! on `format.duration`: a still (PNG, or an AVIF — which probes as codec `av1`
//! exactly like a video) has none; a video does.

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use crate::db::dao::media::MediaKind;

/// Derived facts about a media file — feeds the `media` + `media_variant` rows
/// and the eventual `<source>` / `<img>` / `<object>` tags.
#[derive(Clone, Debug, PartialEq)]
pub struct Probed {
    pub kind: MediaKind,
    pub mime: String,
    /// `<source … codecs="…">` for video (av01… / hvc1); None for images/stl.
    pub codecs: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_ms: Option<i64>,
}

/// Resolve ffprobe once: `$FFPROBE_BIN`, then brew, then PATH (the mini's
/// LaunchAgent PATH excludes /opt/homebrew/bin, so a bare name can't be relied
/// on — same pattern as d2 / weasyprint).
static FFPROBE_BIN: LazyLock<Option<String>> = LazyLock::new(|| resolve_bin("FFPROBE_BIN", "ffprobe"));

pub(crate) fn resolve_bin(env: &str, name: &str) -> Option<String> {
    if let Ok(p) = std::env::var(env)
        && !p.is_empty()
    {
        return Some(p);
    }
    for cand in [
        format!("/opt/homebrew/bin/{name}"),
        format!("/usr/local/bin/{name}"),
    ] {
        if Path::new(&cand).is_file() {
            return Some(cand);
        }
    }
    Command::new(name)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        .then(|| name.to_string())
}

/// Probe a stored file. `original_name` is the upload's filename — used ONLY to
/// spot `.stl` (which ffprobe can't read); the media facts otherwise come from
/// the bytes.
#[allow(dead_code)] // wired by the upload ingest (BZ.5/BZ.7)
pub fn probe(path: &Path, original_name: &str) -> Result<Probed> {
    if original_name.to_ascii_lowercase().ends_with(".stl") {
        return Ok(Probed {
            kind: MediaKind::Stl,
            mime: "model/stl".to_string(),
            codecs: None,
            width: None,
            height: None,
            duration_ms: None,
        });
    }
    let bin = FFPROBE_BIN
        .as_deref()
        .ok_or_else(|| anyhow!("ffprobe not found — `brew install ffmpeg` (looked at $FFPROBE_BIN, /opt/homebrew/bin, /usr/local/bin, PATH)"))?;
    let out = Command::new(bin)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()
        .map_err(|e| anyhow!("failed to spawn ffprobe ({bin}): {e}"))?;
    if !out.status.success() {
        bail!("ffprobe failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_ffprobe(&String::from_utf8_lossy(&out.stdout))
}

#[derive(Deserialize)]
struct FfOut {
    #[serde(default)]
    streams: Vec<FfStream>,
    #[serde(default)]
    format: FfFormat,
}
#[derive(Deserialize)]
struct FfStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    level: Option<i64>,
    pix_fmt: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
}
#[derive(Deserialize, Default)]
struct FfFormat {
    duration: Option<String>, // ffprobe emits this as a STRING ("44.908333")
}

/// Pure parser over ffprobe's JSON — the testable core.
fn parse_ffprobe(json: &str) -> Result<Probed> {
    let out: FfOut = serde_json::from_str(json).map_err(|e| anyhow!("ffprobe json parse: {e}"))?;
    let v = out
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| anyhow!("no video/image stream in ffprobe output"))?;
    let codec = v.codec_name.as_deref().unwrap_or("");

    // A real (>0) duration means it's a video timeline; absent → a still image.
    let duration_secs = out
        .format
        .duration
        .as_deref()
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| *d > 0.0);

    let (kind, mime, codecs) = if duration_secs.is_some() {
        match codec {
            "av1" => (MediaKind::Video, "video/mp4", Some(av1_codecs(v))),
            "hevc" => (MediaKind::Video, "video/mp4", Some("hvc1".to_string())),
            "h264" => (MediaKind::Video, "video/mp4", Some("avc1".to_string())),
            "vp9" => (MediaKind::Video, "video/webm", Some("vp9".to_string())),
            other => bail!("unsupported video codec {other:?}"),
        }
    } else {
        let mime = match codec {
            "av1" => "image/avif",
            "png" => "image/png",
            "mjpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            other => bail!("unsupported image codec {other:?}"),
        };
        (MediaKind::Image, mime, None)
    };

    Ok(Probed {
        kind,
        mime: mime.to_string(),
        codecs,
        width: v.width,
        height: v.height,
        duration_ms: duration_secs.map(|s| (s * 1000.0) as i64),
    })
}

/// `av01.<profile>.<seq_level_idx:02><tier>.<depth>` — profile Main=0, tier Main=M
/// (the common software-encode case). Depth from the pixel format. The level is
/// ffprobe's `level` (the seq_level_idx); browsers accept a well-formed string
/// regardless of whether the level is tight for the resolution.
fn av1_codecs(v: &FfStream) -> String {
    let level = v.level.unwrap_or(0);
    let depth = match v.pix_fmt.as_deref() {
        Some(p) if p.contains("12") => "12",
        Some(p) if p.contains("10") => "10",
        _ => "08",
    };
    format!("av01.0.{level:02}M.{depth}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures match the REAL shapes from skylander dev-data + the cat AVIF
    // (captured via `ffprobe -print_format json`).
    #[test]
    fn av1_video_maps_to_av01_codecs() {
        let json = r#"{"streams":[{"codec_type":"video","codec_name":"av1","level":12,"pix_fmt":"yuv420p","width":1728,"height":1116}],"format":{"duration":"44.908333"}}"#;
        let p = parse_ffprobe(json).unwrap();
        assert_eq!(p.kind, MediaKind::Video);
        assert_eq!(p.mime, "video/mp4");
        assert_eq!(p.codecs.as_deref(), Some("av01.0.12M.08"));
        assert_eq!((p.width, p.height), (Some(1728), Some(1116)));
        assert_eq!(p.duration_ms, Some(44_908));
    }

    #[test]
    fn hevc_video_maps_to_hvc1() {
        let json = r#"{"streams":[{"codec_type":"video","codec_name":"hevc","level":150,"pix_fmt":"yuv420p","width":1728,"height":1116}],"format":{"duration":"44.908333"}}"#;
        let p = parse_ffprobe(json).unwrap();
        assert_eq!(p.kind, MediaKind::Video);
        assert_eq!(p.codecs.as_deref(), Some("hvc1"));
    }

    #[test]
    fn avif_still_is_image_not_video_despite_av1_codec() {
        // AVIF probes as codec av1 with NO duration — the discriminator.
        let json = r#"{"streams":[{"codec_type":"video","codec_name":"av1","pix_fmt":"yuv444p","width":900,"height":681}],"format":{}}"#;
        let p = parse_ffprobe(json).unwrap();
        assert_eq!(p.kind, MediaKind::Image);
        assert_eq!(p.mime, "image/avif");
        assert_eq!(p.codecs, None);
        assert_eq!(p.duration_ms, None);
    }

    #[test]
    fn png_is_image() {
        let json = r#"{"streams":[{"codec_type":"video","codec_name":"png","pix_fmt":"rgba","width":1202,"height":112}],"format":{}}"#;
        let p = parse_ffprobe(json).unwrap();
        assert_eq!(p.kind, MediaKind::Image);
        assert_eq!(p.mime, "image/png");
        assert_eq!(p.codecs, None);
    }

    #[test]
    fn ten_bit_av1_depth() {
        let json = r#"{"streams":[{"codec_type":"video","codec_name":"av1","level":8,"pix_fmt":"yuv420p10le","width":1920,"height":1080}],"format":{"duration":"10.0"}}"#;
        let p = parse_ffprobe(json).unwrap();
        assert_eq!(p.codecs.as_deref(), Some("av01.0.08M.10"));
    }
}
