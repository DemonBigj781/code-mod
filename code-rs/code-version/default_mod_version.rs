pub const DEFAULT_MOD_VERSION: &str = "0.6.197b";
const ORIGINAL_CODE_MOD_VERSION: (u64, u64, u64) = (0, 6, 196);

fn valid_mod_version(version: &str) -> bool {
    let Some(core) = version.strip_suffix('b') else {
        return false;
    };
    let mut parts = core.split('.');
    let Some(major) = parts.next().and_then(parse_version_component) else {
        return false;
    };
    let Some(minor) = parts.next().and_then(parse_version_component) else {
        return false;
    };
    let Some(patch) = parts.next().and_then(parse_version_component) else {
        return false;
    };
    parts.next().is_none() && (major, minor, patch) > ORIGINAL_CODE_MOD_VERSION
}

fn parse_version_component(component: &str) -> Option<u64> {
    if component.is_empty() || (component.len() > 1 && component.starts_with('0')) {
        return None;
    }
    component.parse().ok()
}

fn valid_build_attempt_count(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('0') && value.bytes().all(|byte| byte.is_ascii_digit())
}
