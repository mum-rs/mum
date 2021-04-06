use std::process::Command;

fn main() {
    let version = match commit_hash().as_deref() {
        None | Some("") => format!("v{}", env!("CARGO_PKG_VERSION")),
        Some(version) => version.to_string(),
    };

    println!("cargo:rustc-env=VERSION={}", version);
}

fn commit_hash() -> Option<String> {
    let output = Command::new("git")
        .arg("describe")
        .arg("--tags")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output();
    output
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}
