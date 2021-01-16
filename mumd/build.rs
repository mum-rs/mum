use std::process::Command;

fn main() {
    let version = format!(
        "{} {}-{}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        commit_hash(),
    );

    println!("cargo:rustc-env=VERSION={}", version);
}

fn commit_hash() -> String {
    let output = Command::new("git")
                         .arg("show")
                         .arg("--pretty=format:%h")  // abbrev hash
                         .current_dir(env!("CARGO_MANIFEST_DIR"))
                         .output();
    if let Ok(output) = output {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        String::from("???")
    }
}
