use std::time::Duration;

use anyhow::Result;
use tokio::{process::Command, sync::broadcast::Receiver, time::sleep};
use tracing::{error, info};
pub struct BrowserRelaunchService {}

impl BrowserRelaunchService {
    pub async fn create() -> Result<Self> {
        Ok(Self {})
    }
    pub async fn start(&self, mut endpoints_started: Receiver<()>) -> Result<()> {
        endpoints_started.recv().await?;

        if cfg!(debug_assertions) {
            info!("Got endpoint alert");
            let output = Command::new("osascript")
                .args([
                    "-e",
                    "tell app \"Safari\"",
                    "-e",
                    "activate",
                    "-e",
                    "do JavaScript \"window.location.reload();\" in first document",
                    "-e",
                    "end tell",
                ])
                .output()
                .await?;

            if !output.status.success() {
                error!("Safari notification failed with {}", output.status);
                error!("stdout: {}", String::from_utf8(output.stdout)?);
                error!("stderr: {}", String::from_utf8(output.stderr)?);
            }
        }

        //Sleep forever since these aren't supposed to return
        sleep(Duration::MAX).await;
        Ok(())
    }
}
