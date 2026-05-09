use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tracing::info;

#[derive(Clone, Debug)]
pub struct Settings {
    pub cloudflare_token: String,
    pub domain: String,
    pub database_path: PathBuf,
    pub log_path: PathBuf,
    #[allow(dead_code)]
    pub cache_path: PathBuf,
}

#[derive(Deserialize)]
struct RawSettings {
    cloudflare_token: String,
    domain: String,
    database_path: Option<String>,
    log_path: Option<String>,
    cache_path: Option<String>,
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
        Settings {
            cloudflare_token: raw.cloudflare_token,
            domain: raw.domain,
            database_path: raw
                .database_path
                .map(PathBuf::from)
                .unwrap_or_else(|| app_support.join("data").join("database.sqlite")),
            log_path: raw
                .log_path
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("Library").join("Logs").join("io.hotchkiss.web")),
            cache_path: raw.cache_path.map(PathBuf::from).unwrap_or_else(|| {
                home.join("Library")
                    .join("Caches")
                    .join("io.hotchkiss.web")
            }),
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
                "cache_path": "tc"
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

        Ok(())
    }

    #[test]
    fn defaults_resolve_against_stubbed_home() {
        let home = PathBuf::from("/Users/test");
        let raw = RawSettings {
            cloudflare_token: "ctoken".into(),
            domain: "do".into(),
            database_path: None,
            log_path: None,
            cache_path: None,
        };

        let s = Settings::resolve(raw, &home);
        assert_eq!(s.cloudflare_token, "ctoken");
        assert_eq!(s.domain, "do");
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
    }
}
