use npm_rs::*;
use std::env;

fn main() {
    //Only do an npm prod build in release mode
    if !cfg!(debug_assertions) {
        let out_dir = env::var("OUT_DIR").unwrap();
        println!("cargo:build_dir={}", out_dir); //Path to the build for inclusion in binary

        NpmEnv::default()
            .with_node_env(&NodeEnv::from_cargo_profile().unwrap_or_default())
            .with_env("BUILD_PATH", out_dir)
            .init_env()
            .install(None)
            .run("build")
            .exec()
            .unwrap();
    } else {
        //During development, we will route to npm ourselves
        println!("cargo:rerun-if-changed=build.rs");
    }
}
