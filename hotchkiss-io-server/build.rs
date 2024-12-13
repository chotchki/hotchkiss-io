use anyhow::Result;
use std::{env, process::Command};

fn main() -> Result<()> {
    let schema_key = "DATABASE_URL";
    let schema_url = env::var("DEP_HOTCHKISSIODB_DATABASE_URL").unwrap(); //DEP_HOTCHKISSIODB_
    println!("cargo::rustc-env={}={}", schema_key, schema_url);
    println!("cargo::rerun-if-changed=templates");

    Command::new("npx")
        .args([
            "tailwindcss",
            "-i",
            "styles/tailwind.css",
            "-o",
            "assets/styles/main.css",
        ])
        .output()?;

    Ok(())
}
