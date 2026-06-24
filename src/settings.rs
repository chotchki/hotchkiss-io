use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
};
use tracing::info;

#[derive(Clone, Debug)]
pub struct Settings {
    pub cloudflare_token: String,
    pub domain: String,
    /// WebAuthn relying-party id. Defaults to `domain`, but can be a
    /// registrable parent of it (beta sets `hotchkiss.io` so prod passkeys
    /// authenticate against `beta.hotchkiss.io`).
    pub webauthn_rp_id: String,
    pub database_path: PathBuf,
    pub log_path: PathBuf,
    #[allow(dead_code)]
    pub cache_path: PathBuf,
    /// Directory the daily SQLite snapshots (`database-YYYY-MM-DD.sqlite`) are
    /// written to. Defaults under Application Support; created if missing.
    pub backup_path: PathBuf,
    pub http_port: u16,
    pub https_port: u16,
    pub static_ip: Option<IpAddr>,
}

#[derive(Deserialize)]
struct RawSettings {
    cloudflare_token: String,
    domain: String,
    webauthn_rp_id: Option<String>,
    database_path: Option<String>,
    log_path: Option<String>,
    cache_path: Option<String>,
    backup_path: Option<String>,
    http_port: Option<u16>,
    https_port: Option<u16>,
    static_ip: Option<IpAddr>,
}

impl Settings {
    pub fn load(main_args: impl Iterator<Item = String>) -> Result<Settings> {
        let args: Vec<String> = main_args.skip(1).take(1).collect();
        let home = Self::get_homedir().context("could not resolve home directory")?;

        let config = if args.is_empty() {
            let config_path = Self::make_config_path(&home)?;
            info!("Reading config path {:?}", config_path);
            fs::read_to_string(&config_path)
                .with_context(|| format!("could not open {config_path:?}"))?
        } else {
            info!("Reading env path {:?}", args.first());
            fs::read_to_string(args.first().with_context(|| {
                format!("First argument must be the config file, got {args:?}")
            })?)?
        };

        let raw: RawSettings = serde_json::from_str(&config).with_context(|| {
            format!("Failed to parse settings file to settings struct content:{config}")
        })?;

        Ok(Self::resolve(raw, &home))
    }

    fn resolve(raw: RawSettings, home: &Path) -> Settings {
        let app_support = home
            .join("Library")
            .join("Application Support")
            .join("io.hotchkiss.web");
        let domain = raw.domain;
        Settings {
            cloudflare_token: raw.cloudflare_token,
            webauthn_rp_id: raw.webauthn_rp_id.unwrap_or_else(|| domain.clone()),
            domain,
            database_path: raw
                .database_path
                .map(PathBuf::from)
                .unwrap_or_else(|| app_support.join("data").join("database.sqlite")),
            log_path: raw
                .log_path
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("Library").join("Logs").join("io.hotchkiss.web")),
            cache_path: raw
                .cache_path
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("Library").join("Caches").join("io.hotchkiss.web")),
            backup_path: raw
                .backup_path
                .map(PathBuf::from)
                .unwrap_or_else(|| app_support.join("backups")),
            http_port: raw.http_port.unwrap_or(80),
            https_port: raw.https_port.unwrap_or(443),
            static_ip: raw.static_ip,
        }
    }

    fn make_config_path(parent_path: &Path) -> Result<PathBuf> {
        let mut buffer = parent_path.to_path_buf();
        buffer.push("Library");
        buffer.push("Application Support");
        buffer.push("io.hotchkiss.web");

        fs::DirBuilder::new().recursive(true).create(&buffer)?;

        buffer.push("config.json");

        Ok(buffer)
    }

    fn get_homedir() -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            use objc2_foundation::NSHomeDirectory;

            //SAFETY: Constant string as per Apple's documentation
            unsafe {
                let home_dir_string = NSHomeDirectory();
                Some(PathBuf::from(home_dir_string.to_string()))
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            std::env::home_dir()
        }
    }
}

#[cfg(test)]
mod test {
    use std::io::Write;

    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn load_with_explicit_paths() -> Result<()> {
        let mut file = NamedTempFile::new()?;

        writeln!(
            file,
            r#"
            {{
                "cloudflare_token": "ctoken",
                "database_path": "dp",
                "domain": "do",
                "log_path": "t",
                "cache_path": "tc",
                "backup_path": "bp"
            }}
            "#
        )?;

        let args: Vec<String> = vec![" ".into(), file.path().to_string_lossy().to_string()];

        let s = Settings::load(args.into_iter()).unwrap();
        assert_eq!(s.cloudflare_token, "ctoken");
        assert_eq!(s.database_path, PathBuf::from("dp"));
        assert_eq!(s.domain, "do");
        assert_eq!(s.log_path, PathBuf::from("t"));
        assert_eq!(s.cache_path, PathBuf::from("tc"));
        assert_eq!(s.backup_path, PathBuf::from("bp"));
        assert_eq!(s.http_port, 80);
        assert_eq!(s.https_port, 443);

        Ok(())
    }

    #[test]
    fn load_with_custom_ports() -> Result<()> {
        let mut file = NamedTempFile::new()?;

        writeln!(
            file,
            r#"
            {{
                "cloudflare_token": "ctoken",
                "domain": "do",
                "http_port": 8080,
                "https_port": 8443
            }}
            "#
        )?;

        let args: Vec<String> = vec![" ".into(), file.path().to_string_lossy().to_string()];

        let s = Settings::load(args.into_iter()).unwrap();
        assert_eq!(s.http_port, 8080);
        assert_eq!(s.https_port, 8443);
        // rp_id defaults to the domain when omitted
        assert_eq!(s.webauthn_rp_id, "do");

        Ok(())
    }

    #[test]
    fn defaults_resolve_against_stubbed_home() {
        let home = PathBuf::from("/Users/test");
        let raw = RawSettings {
            cloudflare_token: "ctoken".into(),
            domain: "do".into(),
            webauthn_rp_id: None,
            database_path: None,
            log_path: None,
            cache_path: None,
            backup_path: None,
            http_port: None,
            https_port: None,
            static_ip: None,
        };

        let s = Settings::resolve(raw, &home);
        assert_eq!(s.cloudflare_token, "ctoken");
        assert_eq!(s.domain, "do");
        assert_eq!(s.webauthn_rp_id, "do");
        assert_eq!(
            s.database_path,
            PathBuf::from(
                "/Users/test/Library/Application Support/io.hotchkiss.web/data/database.sqlite"
            ),
        );
        assert_eq!(
            s.log_path,
            PathBuf::from("/Users/test/Library/Logs/io.hotchkiss.web"),
        );
        assert_eq!(
            s.cache_path,
            PathBuf::from("/Users/test/Library/Caches/io.hotchkiss.web"),
        );
        assert_eq!(
            s.backup_path,
            PathBuf::from("/Users/test/Library/Application Support/io.hotchkiss.web/backups"),
        );
        assert_eq!(s.http_port, 80);
        assert_eq!(s.https_port, 443);
        assert!(s.static_ip.is_none());
    }

    #[test]
    fn load_with_static_ip() -> Result<()> {
        let mut file = NamedTempFile::new()?;

        writeln!(
            file,
            r#"
            {{
                "cloudflare_token": "ctoken",
                "domain": "do",
                "static_ip": "192.168.1.42"
            }}
            "#
        )?;

        let args: Vec<String> = vec![" ".into(), file.path().to_string_lossy().to_string()];

        let s = Settings::load(args.into_iter()).unwrap();
        assert_eq!(s.static_ip, Some("192.168.1.42".parse().unwrap()));

        Ok(())
    }

    #[test]
    fn load_with_webauthn_rp_id() -> Result<()> {
        let mut file = NamedTempFile::new()?;

        writeln!(
            file,
            r#"
            {{
                "cloudflare_token": "ctoken",
                "domain": "beta.hotchkiss.io",
                "webauthn_rp_id": "hotchkiss.io"
            }}
            "#
        )?;

        let args: Vec<String> = vec![" ".into(), file.path().to_string_lossy().to_string()];

        let s = Settings::load(args.into_iter()).unwrap();
        assert_eq!(s.domain, "beta.hotchkiss.io");
        assert_eq!(s.webauthn_rp_id, "hotchkiss.io");

        Ok(())
    }
}
