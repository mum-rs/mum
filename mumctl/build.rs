use std::process::Command;

fn main() {
    let version = format!(
        "{}-{}",
        env!("CARGO_PKG_VERSION"),
        commit_hash().unwrap_or_else(|| "???".to_string()),
    );

    println!("cargo:rustc-env=VERSION={}", version);
}

fn commit_hash() -> Option<String> {
    let output = Command::new("git")
                         .arg("show")
                         .arg("--pretty=format:%h")  // abbrev hash
                         .current_dir(env!("CARGO_MANIFEST_DIR"))
                         .output();
    output.ok().map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}
