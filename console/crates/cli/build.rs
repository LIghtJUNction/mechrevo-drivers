include!("../../resources/windows/embed-gnu-manifest.rs");

fn main() {
    embed_gnu_manifests(&[
        ("mechrevo-console", "console-cli.manifest"),
        ("mechrevo-console-oem-helper", "console-oem-helper.manifest"),
    ]);
}
