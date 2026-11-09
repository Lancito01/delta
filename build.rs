//! Embeds the app icon and version details into delta.exe.

fn main() {
    let v = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<&str> = v.split(['.', '-']).take(3).collect();
    parts.resize(3, "0");
    let commas = format!("{},{},{},0", parts[0], parts[1], parts[2]);
    let macros = [format!("DELTA_VERSION_COMMAS={commas}"), format!("DELTA_VERSION_STRING=\"{v}\"")];
    embed_resource::compile("assets/delta.rc", &macros).manifest_optional().unwrap();
    println!("cargo:rerun-if-changed=assets/delta.rc");
    println!("cargo:rerun-if-changed=assets/delta.ico");
}
