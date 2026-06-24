//! Build script: generate a compile-time SPDX license-text lookup table from
//! `assets/licenses/*.txt`, embedding each text in the binary (FR-017).

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let licenses_dir = Path::new(&manifest_dir).join("assets/licenses");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let dest = Path::new(&out_dir).join("licet_licenses.rs");

    println!("cargo:rerun-if-changed=assets/licenses");

    let mut entries: Vec<String> = Vec::new();
    if licenses_dir.is_dir() {
        let mut files: Vec<_> = fs::read_dir(&licenses_dir)
            .expect("read assets/licenses")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "txt").unwrap_or(false))
            .collect();
        files.sort();
        for path in files {
            println!("cargo:rerun-if-changed={}", path.display());
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("license file stem")
                .to_string();
            let abs = path.canonicalize().expect("canonicalize license path");
            entries.push(format!(
                "    ({id:?}, include_str!({path:?})),",
                id = id,
                path = abs.display().to_string()
            ));
        }
    }

    // Point-in-time SPDX list snapshot identifier surfaced via --version / lint (FR-028).
    let list_version =
        env::var("LICET_SPDX_LIST_VERSION").unwrap_or_else(|_| "3.25-bundled".to_string());

    let generated = format!(
        "/// Embedded SPDX license texts, keyed by SPDX identifier (sorted).\n\
         pub static BUNDLED_LICENSES: &[(&str, &str)] = &[\n{}\n];\n\n\
         /// Version of the embedded SPDX license-list snapshot (FR-028).\n\
         pub const SPDX_LIST_VERSION: &str = {:?};\n",
        entries.join("\n"),
        list_version
    );

    fs::write(&dest, generated).expect("write generated licenses table");
}
