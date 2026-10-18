include!("../../resources/windows/embed-gnu-manifest.rs");

fn main() {
    embed_gnu_manifests(&[("mechrevo-console-gui", "console-gui.manifest")]);
}
