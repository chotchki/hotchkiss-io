use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    env::{self},
    fs,
    path::{Path, PathBuf},
};
use tracing::info;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub cloudflare_token: String,
    pub database_path: String,
    pub domain: String,
    pub log_path: String,
    pub cache_path: String,
}

impl Settings {
    pub fn load(main_args: impl Iterator<Item = String>) -> Result<Settings> {
        let args: Vec<String> = main_args.skip(1).take(1).collect();

        let config = if args.is_empty()
            && let Some(home) = Self::get_homedir()
        {
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

        let settings: Settings = serde_json::from_str(&config).with_context(|| {
            format!("Failed to parse settings file to settings struct content:{config}")
        })?;

        Ok(settings)
    }

    fn make_config_path(parent_path: &Path) -> Result<PathBuf> {
        let mut buffer = parent_path.to_path_buf();
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
            env::home_dir()
        }
    }
}

#[cfg(test)]
mod test {
    use std::io::Write;

    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn load_test_args() -> Result<()> {
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
        assert_eq!(s.database_path, "dp");
        assert_eq!(s.domain, "do");
        assert_eq!(s.log_path, "t");
        assert_eq!(s.cache_path, "tc");

        Ok(())
    }
}
