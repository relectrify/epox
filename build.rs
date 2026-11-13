fn main() {
    // refuse to compile if the `nightly` feature is requested with a stable
    // toolchain
    let compiler_version = rustc_version::version_meta().unwrap();
    let can_use_nightly_features = match compiler_version.channel {
        rustc_version::Channel::Beta | rustc_version::Channel::Stable => false,
        rustc_version::Channel::Dev | rustc_version::Channel::Nightly => true,
    };

    // enable the never type for rust >= 1.100.0 without bumping the crate MSRV
    println!("cargo::rustc-check-cfg=cfg(never_type_stable)");

    if compiler_version.semver >= "1.100.0".parse().unwrap() {
        println!("cargo::rustc-cfg=never_type_stable");
    }

    if std::env::var("CARGO_FEATURE_NIGHTLY").is_ok() && !can_use_nightly_features {
        println!("cargo::error=\"'nightly' feature enabled without a nightly toolchain\"");
    }
}
