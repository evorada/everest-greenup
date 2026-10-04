use everestrs_build::Builder;

/// EVerest's CMake sets EVEREST_CORE_ROOT; the host workspace points at
/// the `module/everest-core` symlink.
pub fn main() {
    let root = std::env::var("EVEREST_CORE_ROOT").unwrap_or_else(|_| "../everest-core".to_string());
    if let Err(e) = Builder::new("manifest.yaml", vec![root]).generate() {
        panic!("everestrs code generation failed: {e}");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=manifest.yaml");
    println!("cargo:rerun-if-env-changed=EVEREST_CORE_ROOT");
}
