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

use crate::db::dao::media::{MediaKind, SCAD_MIME};

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
    /// Audio chapters as JSON `[{"start_ms": N, "title": "…"}]` (Phase DD) —
    /// only set for `MediaKind::Audio` with at least one chapter.
    pub chapters: Option<String>,
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
    let lower = original_name.to_ascii_lowercase();
    if lower.ends_with(".stl") {
        return Ok(Probed {
            kind: MediaKind::Stl,
            mime: "model/stl".to_string(),
            codecs: None,
            width: None,
            height: None,
            duration_ms: None,
            chapters: None,
        });
    }
    // `.3mf` is a VIEWABLE model (same kind as STL), with a `model/3mf` mime so the
    // render can prefer it for the viewer (it carries COLOR, unlike STL) — whether
    // it's the whole model item or a variant alongside an STL LOD. Plates stay
    // downloads NOT via kind but because `fab` lists them as plain markdown LINKS,
    // not `![]` embeds — only an embed hits this kind dispatch. ffprobe can't read
    // the zip container. (`MediaKind::Stl` is the viewable-mesh kind; it covers 3MF.)
    if lower.ends_with(".3mf") {
        return Ok(Probed {
            kind: MediaKind::Stl,
            mime: "model/3mf".to_string(),
            codecs: None,
            width: None,
            height: None,
            duration_ms: None,
            chapters: None,
        });
    }
    // `.scad` is OpenSCAD SOURCE — the input fab-gui SLICES, NOT a viewable mesh
    // (three.js can't render it, so it must NOT be `MediaKind::Stl` or it'd hit the
    // mesh viewer). Stored as a downloadable `File` tagged `application/x-openscad`
    // so the embed dispatch offers "Open in the slicer" (Phase DN). Grouped with a
    // mesh, `dominant_kind` keeps the ITEM `Stl`; the scad rides as a variant.
    // ffprobe can't read it either.
    if lower.ends_with(".scad") {
        return Ok(Probed {
            kind: MediaKind::File,
            mime: SCAD_MIME.to_string(),
            codecs: None,
            width: None,
            height: None,
            duration_ms: None,
            chapters: None,
        });
    }
    // `.epub` is a book (manga / novel) the foliate-js reader renders in-browser
    // (Phase DV) — typed by extension like the other zip-container formats, since
    // ffprobe can't read the zip. Served as a plain `application/epub+zip` blob
    // (inert → the embed's `MediaKind::Epub` branch mounts the reader).
    if lower.ends_with(".epub") {
        return Ok(Probed {
            kind: MediaKind::Epub,
            mime: "application/epub+zip".to_string(),
            codecs: None,
            width: None,
            height: None,
            duration_ms: None,
            chapters: None,
        });
    }
    // `.cbz` (comic-book zip — a zip of page images) is read by the SAME foliate reader
    // (Phase DW.8), which dispatches on the `application/vnd.comicbook+zip` mime. Typed
    // by extension like `.epub` (ffprobe can't read the zip). The cover is the first
    // image in the zip (extracted in `add_derived_variants`).
    if lower.ends_with(".cbz") {
        return Ok(Probed {
            kind: MediaKind::Cbz,
            mime: "application/vnd.comicbook+zip".to_string(),
            codecs: None,
            width: None,
            height: None,
            duration_ms: None,
            chapters: None,
        });
    }
    let bin = FFPROBE_BIN
        .as_deref()
        .ok_or_else(|| anyhow!("ffprobe not found — `brew install ffmpeg` (looked at $FFPROBE_BIN, /opt/homebrew/bin, /usr/local/bin, PATH)"))?;
    let out = Command::new(bin)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams", "-show_chapters"])
        .arg(path)
        .output()
        .map_err(|e| anyhow!("failed to spawn ffprobe ({bin}): {e}"))?;
    // ffprobe RAN but couldn't read it as media (non-zero exit) → a generic file
    // (zip / app / binary / …): store it as a downloadable MediaKind::File rather
    // than rejecting the upload. (A missing or un-spawnable ffprobe still errors
    // loudly above — that's a deploy misconfig, not a non-media file.)
    if !out.status.success() {
        return Ok(generic_file(original_name));
    }
    // Parsed, but no usable A/V stream / an unsupported codec → also a generic file.
    Ok(parse_ffprobe(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|_| generic_file(original_name)))
}

/// A non-A/V upload (ffprobe can't type it) → a downloadable generic file. The mime
/// is guessed from the filename extension (octet-stream fallback) so the byte route
/// serves a sensible Content-Type — the browser downloads an archive, inlines a PDF,
/// etc. — instead of the upload being rejected.
fn generic_file(original_name: &str) -> Probed {
    let mime = mime_guess::from_path(original_name)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string();
    Probed {
        kind: MediaKind::File,
        mime,
        codecs: None,
        width: None,
        height: None,
        duration_ms: None,
        chapters: None,
    }
}

#[derive(Deserialize)]
struct FfOut {
    #[serde(default)]
    streams: Vec<FfStream>,
    #[serde(default)]
    format: FfFormat,
    #[serde(default)]
    chapters: Vec<FfChapter>,
}
#[derive(Deserialize)]
struct FfStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    level: Option<i64>,
    pix_fmt: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    #[serde(default)]
    disposition: Option<FfDisposition>,
}
/// The stream disposition flags — `attached_pic: 1` marks embedded COVER ART
/// (an m4b/mp3's art ships as an mjpeg/png "video" stream), the guard that
/// keeps an audiobook from misreading as a video.
#[derive(Deserialize)]
struct FfDisposition {
    attached_pic: Option<i64>,
}
#[derive(Deserialize)]
struct FfChapter {
    start_time: Option<String>, // seconds, as a string like format.duration
    tags: Option<FfChapterTags>,
}
#[derive(Deserialize)]
struct FfChapterTags {
    title: Option<String>,
}
#[derive(Deserialize, Default)]
struct FfFormat {
    duration: Option<String>, // ffprobe emits this as a STRING ("44.908333")
}

/// Pure parser over ffprobe's JSON — the testable core.
fn parse_ffprobe(json: &str) -> Result<Probed> {
    let out: FfOut = serde_json::from_str(json).map_err(|e| anyhow!("ffprobe json parse: {e}"))?;

    // A real (>0) duration means a playable timeline; absent → a still image.
    let duration_secs = out
        .format
        .duration
        .as_deref()
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| *d > 0.0);

    // A REAL video stream — embedded cover art (an m4b/mp3's mjpeg/png art
    // ships as a "video" stream flagged attached_pic) does NOT count, or every
    // audiobook with art would misread as an unsupported video (Phase DD).
    let real_video = out.streams.iter().find(|s| {
        s.codec_type.as_deref() == Some("video")
            && s.disposition
                .as_ref()
                .is_none_or(|d| d.attached_pic != Some(1))
    });

    // No real video → an AUDIO stream classifies the file (Phase DD).
    let Some(v) = real_video else {
        return parse_audio(&out, duration_secs);
    };
    let codec = v.codec_name.as_deref().unwrap_or("");

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
        chapters: None,
    })
}

/// The audio arm (Phase DD): UNIVERSAL-codec map ONLY — aac/mp3/flac play on
/// every device in the stated mixed-Apple/other family audience. Opus/Vorbis
/// (never in Safari) and ALAC (only in Safari) bail → the caller degrades the
/// upload to a downloadable `MediaKind::File` rather than minting a player
/// that errors for half the household. AAC m4b is the canonical format.
fn parse_audio(out: &FfOut, duration_secs: Option<f64>) -> Result<Probed> {
    let a = out
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"))
        .ok_or_else(|| anyhow!("no video or audio stream in ffprobe output"))?;
    let mime = match a.codec_name.as_deref().unwrap_or("") {
        "aac" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        other => bail!("non-universal audio codec {other:?}"),
    };
    Ok(Probed {
        kind: MediaKind::Audio,
        mime: mime.to_string(),
        codecs: None,
        width: None,
        height: None,
        duration_ms: duration_secs.map(|s| (s * 1000.0) as i64),
        chapters: chapters_json(&out.chapters),
    })
}

/// ffprobe chapters → the stored JSON `[{"start_ms": N, "title": "…"}]`.
/// `None` when there are no chapters (a chapterless mp3 needs no list UI).
fn chapters_json(chapters: &[FfChapter]) -> Option<String> {
    if chapters.is_empty() {
        return None;
    }
    let entries: Vec<serde_json::Value> = chapters
        .iter()
        .map(|c| {
            let start_ms = c
                .start_time
                .as_deref()
                .and_then(|t| t.parse::<f64>().ok())
                .map(|s| (s * 1000.0) as i64)
                .unwrap_or(0);
            let title = c
                .tags
                .as_ref()
                .and_then(|t| t.title.clone())
                .unwrap_or_default();
            serde_json::json!({"start_ms": start_ms, "title": title})
        })
        .collect();
    serde_json::to_string(&entries).ok()
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

    // `.stl` / `.3mf` are typed by extension BEFORE ffprobe runs, so these need no
    // binary or real file.
    #[test]
    fn stl_and_3mf_are_typed_by_extension() {
        let stl = probe(Path::new("/nope"), "part.STL").unwrap();
        assert_eq!(stl.kind, MediaKind::Stl);
        assert_eq!(stl.mime, "model/stl");
        // 3MF is a VIEWABLE model (kind Stl covers viewable meshes) carrying color,
        // with a recognizable mime so the render prefers it for the viewer.
        let mf = probe(Path::new("/nope"), "plate.3mf").unwrap();
        assert_eq!(mf.kind, MediaKind::Stl);
        assert_eq!(mf.mime, "model/3mf");
    }

    // `.epub` is a book (Phase DV): typed by extension (ffprobe can't read the zip),
    // kind Epub, mime application/epub+zip — the embed mounts the foliate reader.
    #[test]
    fn epub_is_typed_by_extension() {
        let ep = probe(Path::new("/nope"), "MangaSeries v01.EPUB").unwrap();
        assert_eq!(ep.kind, MediaKind::Epub);
        assert_eq!(ep.mime, "application/epub+zip");
    }

    // `.scad` is OpenSCAD SOURCE (Phase DN): a downloadable File, NOT a mesh kind,
    // tagged application/x-openscad so the embed can offer "Open in the slicer".
    #[test]
    fn scad_is_typed_as_openscad_source_by_extension() {
        let scad = probe(Path::new("/nope"), "bracket.SCAD").unwrap();
        assert_eq!(scad.kind, MediaKind::File);
        assert_eq!(scad.mime, SCAD_MIME);
        assert_eq!(scad.mime, "application/x-openscad");
    }

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
    fn generic_file_is_downloadable_file_kind_with_guessed_mime() {
        // A non-A/V upload → MediaKind::File, mime guessed from the extension.
        let zip = generic_file("demo-build.zip");
        assert_eq!(zip.kind, MediaKind::File);
        assert_eq!(zip.mime, "application/zip");
        assert!(zip.codecs.is_none());
        assert_eq!((zip.width, zip.height, zip.duration_ms), (None, None, None));

        // Unknown extension → octet-stream (browser downloads it).
        let unknown = generic_file("mystery.zzz");
        assert_eq!(unknown.kind, MediaKind::File);
        assert_eq!(unknown.mime, "application/octet-stream");
    }

    /// DD.1 KAT against the REAL fixture (aac + attached_pic mjpeg cover + two
    /// chapters, generated by ffmpeg — the exact shape of a real m4b). Runs the
    /// actual ffprobe, which is a hard project dependency like d2/weasyprint.
    #[test]
    fn m4b_fixture_probes_as_audio_with_chapters() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chapters.m4b");
        let p = probe(&path, "chapters.m4b").unwrap();
        assert_eq!(p.kind, MediaKind::Audio, "cover art must not misread as video");
        assert_eq!(p.mime, "audio/mp4");
        assert_eq!(p.codecs, None);
        assert_eq!(p.duration_ms, Some(2_000));
        let chapters: serde_json::Value =
            serde_json::from_str(p.chapters.as_deref().expect("chapters stamped")).unwrap();
        let ch = chapters.as_array().unwrap();
        assert_eq!(ch.len(), 2);
        assert_eq!(ch[0]["start_ms"], 0);
        assert_eq!(ch[0]["title"], "Chapter One");
        assert_eq!(ch[1]["start_ms"], 1000);
        assert_eq!(ch[1]["title"], "Chapter Two");
    }

    /// The attached_pic guard, pure-JSON: an audio file whose cover art ships
    /// as a "video" stream must classify by the AUDIO stream.
    #[test]
    fn attached_pic_cover_does_not_misread_as_video() {
        let json = r#"{"streams":[
            {"codec_type":"audio","codec_name":"aac"},
            {"codec_type":"video","codec_name":"mjpeg","width":64,"height":64,"disposition":{"attached_pic":1}}
        ],"format":{"duration":"2.000000"},"chapters":[]}"#;
        let p = parse_ffprobe(json).unwrap();
        assert_eq!(p.kind, MediaKind::Audio);
        assert_eq!(p.mime, "audio/mp4");
        assert_eq!((p.width, p.height), (None, None), "cover dims are not the item's dims");
        assert_eq!(p.chapters, None, "no chapters → no list");
    }

    /// UNIVERSAL-only codec map: mp3/flac in; opus (never Safari) and alac
    /// (only Safari) bail → the caller degrades to a downloadable File instead
    /// of a player that errors for half the family.
    #[test]
    fn audio_codec_map_is_universal_only() {
        let probe_codec = |codec: &str| {
            parse_ffprobe(&format!(
                r#"{{"streams":[{{"codec_type":"audio","codec_name":"{codec}"}}],"format":{{"duration":"3.0"}}}}"#
            ))
        };
        assert_eq!(probe_codec("mp3").unwrap().mime, "audio/mpeg");
        assert_eq!(probe_codec("flac").unwrap().mime, "audio/flac");
        assert!(probe_codec("opus").is_err());
        assert!(probe_codec("vorbis").is_err());
        assert!(probe_codec("alac").is_err());
    }

    /// A REAL video with an audio track stays a video — the guard only skips
    /// attached_pic streams, not genuine video.
    #[test]
    fn real_video_with_audio_track_stays_video() {
        let json = r#"{"streams":[
            {"codec_type":"audio","codec_name":"aac"},
            {"codec_type":"video","codec_name":"hevc","level":150,"pix_fmt":"yuv420p","width":1920,"height":1080,"disposition":{"attached_pic":0}}
        ],"format":{"duration":"10.0"}}"#;
        let p = parse_ffprobe(json).unwrap();
        assert_eq!(p.kind, MediaKind::Video);
        assert_eq!(p.codecs.as_deref(), Some("hvc1"));
    }

    #[test]
    fn ten_bit_av1_depth() {
        let json = r#"{"streams":[{"codec_type":"video","codec_name":"av1","level":8,"pix_fmt":"yuv420p10le","width":1920,"height":1080}],"format":{"duration":"10.0"}}"#;
        let p = parse_ffprobe(json).unwrap();
        assert_eq!(p.codecs.as_deref(), Some("av01.0.08M.10"));
    }
}
