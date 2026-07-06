//! Load + prepare the toll image for the challenge (CX.4).
//!
//! At boot the server decodes the committed `assets/greylist/toll.png`, forces every pixel
//! opaque, and precomputes the image-chain digest + version. The RGBA buffer is what the
//! challenge endpoint SHIPS (as a static asset) AND what the digest is computed over — never the
//! PNG, so the client's decoded bytes match the server's exactly (design doc: "derived raw RGBA").

use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use rust_embed::RustEmbed;

use crate::greylist::challenge::{image_digest, version_tag};

/// Process-wide toll image, decoded + digested ONCE (the ~1s chain over ~188k pixels is paid a
/// single time, not per test server / per boot path). `expect` is fine: a load failure means the
/// committed `toll.png` is missing/corrupt, which is a build problem, not a runtime one.
static SHARED: LazyLock<Arc<TollImage>> =
    LazyLock::new(|| Arc::new(TollImage::load().expect("decode embedded assets/greylist/toll.png")));

/// Only the greylist assets — a dedicated embed so we don't expose or duplicate the site-wide
/// `assets/` embed. Small (just `toll.png`).
#[derive(RustEmbed)]
#[folder = "assets/greylist"]
struct GreylistAsset;

/// The decoded, opaque toll image plus its precomputed digest + version. Built once at boot and
/// shared read-only.
#[derive(Clone)]
pub struct TollImage {
    /// Row-major RGBA, fully opaque. Served to the client verbatim; hashed to `digest`.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// `image_digest(&rgba)` — precomputed so per-request issue/verify is cheap.
    pub digest: [u8; 32],
    /// Short content-hash tag bound into each challenge seed.
    pub version: String,
}

impl TollImage {
    /// Decode the embedded `toll.png` → opaque RGBA → digest + version.
    pub fn load() -> Result<TollImage> {
        let file =
            GreylistAsset::get("toll.png").context("assets/greylist/toll.png not embedded")?;
        let img = image::load_from_memory(&file.data)
            .context("decoding toll.png")?
            .to_rgba8();
        let (width, height) = img.dimensions();
        let mut rgba = img.into_raw();
        // Force fully opaque so the client's putImageData<->getImageData round-trip is byte-exact.
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
        let digest = image_digest(&rgba);
        let version = version_tag(&rgba);
        Ok(TollImage {
            width,
            height,
            digest,
            version,
            rgba,
        })
    }

    /// The process-shared toll image (decoded once). Cheap `Arc` clone.
    pub fn shared() -> Arc<TollImage> {
        SHARED.clone()
    }
}

impl std::fmt::Debug for TollImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TollImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("version", &self.version)
            .field("rgba_bytes", &self.rgba.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_committed_toll_image_opaque_and_consistent() {
        let t = TollImage::load().expect("toll.png decodes");
        assert!(t.width > 0 && t.height > 0);
        assert_eq!(
            t.rgba.len() as u32,
            t.width * t.height * 4,
            "RGBA buffer length matches dimensions"
        );
        assert!(
            t.rgba.chunks_exact(4).all(|px| px[3] == 255),
            "every pixel forced fully opaque"
        );
        assert_eq!(t.digest.len(), 32);
        assert_eq!(
            t.digest,
            image_digest(&t.rgba),
            "stored digest is the chain over the served buffer"
        );
        assert!(!t.version.is_empty());
    }
}
