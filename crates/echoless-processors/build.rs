use std::env;
use std::path::PathBuf;

const SOURCES: &[&str] = &[
    "denoise.c",
    "rnn.c",
    "pitch.c",
    "kiss_fft.c",
    "celt_lpc.c",
    "nnet.c",
    "nnet_default.c",
    "rnnoise_model.c",
    "parse_lpcnet_weights.c",
    "rnnoise_tables.c",
];

fn main() {
    let vendor = PathBuf::from("vendor/rnnoise");
    let source_dir = vendor.join("src");
    let target = env::var("TARGET").expect("Cargo must set TARGET");

    let mut build = cc::Build::new();
    build
        .include(vendor.join("include"))
        .include(&source_dir)
        .define("RNNOISE_BUILD", None)
        .define("USE_WEIGHTS_FILE", None)
        .define("DISABLE_DEBUG_FLOAT", None)
        .flag_if_supported("-std=c99")
        .flag_if_supported("/std:c11")
        .warnings(false);

    if target.contains("apple-darwin") {
        build.flag_if_supported("-mmacosx-version-min=11.0");
    }

    for source in SOURCES {
        build.file(source_dir.join(source));
    }

    build.compile("echoless_rnnoise");

    if !target.contains("windows") {
        println!("cargo:rustc-link-lib=m");
    }
    println!("cargo:rerun-if-changed={}", vendor.display());
}
