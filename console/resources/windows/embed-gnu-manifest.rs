// Shared by the two Cargo build scripts. MSVC packaging embeds and verifies
// these same manifests with mt.exe; GNU builds need a linked resource object.
fn embed_gnu_manifests(entries: &[(&str, &str)]) {
    use std::{env, fs, path::PathBuf, process::Command};
    for (_, manifest) in entries {
        println!("cargo:rerun-if-changed=../../resources/windows/{manifest}");
    }
    println!("cargo:rerun-if-env-changed=WINDRES");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("gnu")
    {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_ARCH").as_deref(),
        Ok("x86_64"),
        "Windows console packaging currently supports x64"
    );
    let resources =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../resources/windows");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let compiler = env::var_os("WINDRES").unwrap_or_else(|| "x86_64-w64-mingw32-windres".into());
    for (binary, name) in entries {
        let manifest = resources
            .join(name)
            .canonicalize()
            .expect("Fixed executable manifest is missing");
        let path = manifest.to_str().expect("Manifest path must be UTF-8");
        assert!(
            !path.chars().any(char::is_control),
            "Manifest path contains a control character"
        );
        let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
        let rc = out.join(format!("{binary}.rc"));
        let object = out.join(format!("{binary}.manifest.o"));
        fs::write(&rc, format!("1 24 \"{escaped}\"\n"))
            .expect("Cannot write manifest resource description");
        let status = Command::new(&compiler)
            .args([
                "--codepage=65001",
                "--input-format=rc",
                "--output-format=coff",
            ])
            .arg("--input")
            .arg(&rc)
            .arg("--output")
            .arg(&object)
            .status()
            .expect("Cannot run the Windows resource compiler");
        assert!(status.success(), "Windows resource compiler failed");
        println!("cargo:rustc-link-arg-bin={binary}={}", object.display());
    }
}
