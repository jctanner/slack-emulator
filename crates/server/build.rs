// rust-embed needs the SPA directory to exist at compile time. Create an empty
// one so the server builds before the frontend has been built.
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    std::fs::create_dir_all(&dist).expect("create web/dist");
    println!("cargo:rerun-if-changed=build.rs");
}
