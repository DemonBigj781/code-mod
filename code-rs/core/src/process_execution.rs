const COSMOPOLITAN_PROCESS_TOOL_ERROR: &str = "External process tools are disabled in this Cosmopolitan APE build until equivalent host sandboxing is verified.";

pub(crate) fn unsupported_process_tool_message() -> Option<&'static str> {
    if cfg!(feature = "cosmopolitan-ape") {
        Some(COSMOPOLITAN_PROCESS_TOOL_ERROR)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::unsupported_process_tool_message;

    #[test]
    fn process_tool_gate_matches_the_cosmopolitan_build_profile() {
        assert_eq!(
            unsupported_process_tool_message().is_some(),
            cfg!(feature = "cosmopolitan-ape")
        );
    }
}
