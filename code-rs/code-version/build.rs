include!("default_version.rs");
include!("default_mod_version.rs");

fn main() {
    // Prefer an explicit CODE_VERSION provided by CI; otherwise embed the
    // pinned stable CLI version used by local builds.
    let version =
        std::env::var("CODE_VERSION").unwrap_or_else(|_| DEFAULT_CODE_VERSION.to_string());
    let mod_version = std::env::var("CODE_MOD_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MOD_VERSION.to_string());
    let build_attempt = std::env::var("CODE_BUILD_ATTEMPT_COUNT")
        .ok()
        .filter(|value| !value.trim().is_empty());

    if !valid_mod_version(&mod_version) {
        panic!("CODE_MOD_VERSION must use X.Y.Zb and be newer than 0.6.196");
    }
    if let Some(attempt) = build_attempt.as_deref()
        && !valid_build_attempt_count(attempt)
    {
        panic!("CODE_BUILD_ATTEMPT_COUNT must be a positive integer");
    }

    // Inject the version as a rustc env so it participates in the compiler
    // invocation hash (sccache-friendly) and guarantees a cache miss when
    // the version changes.
    println!("cargo:rustc-env=CODE_VERSION={version}");
    println!("cargo:rustc-env=CODE_MOD_VERSION={mod_version}");
    if let Some(attempt) = build_attempt {
        println!("cargo:rustc-env=CODE_BUILD_ATTEMPT_COUNT={attempt}");
    }

    // Ensure dependent crates rebuild when CODE_VERSION changes even if the
    // source graph stays the same.
    println!("cargo:rerun-if-env-changed=CODE_VERSION");
    println!("cargo:rerun-if-env-changed=CODE_MOD_VERSION");
    println!("cargo:rerun-if-env-changed=CODE_BUILD_ATTEMPT_COUNT");
}
