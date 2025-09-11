fn main() {
    // refuse to compile if the `nightly` feature is requested with a stable
    // toolchain
    let can_use_nightly_features = match rustc_version::version_meta().unwrap().channel {
        rustc_version::Channel::Beta | rustc_version::Channel::Stable => false,
        rustc_version::Channel::Dev | rustc_version::Channel::Nightly => true,
    };

    if std::env::var("CARGO_FEATURE_NIGHTLY").is_ok() && !can_use_nightly_features {
        println!("cargo::error=\"'nightly' feature enabled without a nightly toolchain\"");
    }
}
