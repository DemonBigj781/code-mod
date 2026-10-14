#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TabCommand {
    List,
    Next,
    Previous,
    Select(usize),
    Close(Option<usize>),
    New,
    Restore,
}

pub(crate) const TAB_USAGE: &str =
    "/tab next|prev|N|close [N]|new; /tabs restore (or /tabs to list)";

pub(crate) fn parse_tab_command(args: &str) -> Result<TabCommand, &'static str> {
    let words: Vec<_> = args.split_whitespace().collect();
    let ordinal = |word: &str| word.parse::<usize>().ok().filter(|n| *n > 0);
    match words.as_slice() {
        [] | ["list"] => Ok(TabCommand::List),
        ["next"] => Ok(TabCommand::Next),
        ["prev" | "previous"] => Ok(TabCommand::Previous),
        ["new"] => Ok(TabCommand::New),
        ["restore"] => Ok(TabCommand::Restore),
        ["close"] => Ok(TabCommand::Close(None)),
        ["close", n] => ordinal(n)
            .map(|n| TabCommand::Close(Some(n)))
            .ok_or(TAB_USAGE),
        [n] => ordinal(n).map(TabCommand::Select).ok_or(TAB_USAGE),
        _ => Err(TAB_USAGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_commands_and_invalid_ordinals() {
        for (input, expected) in [
            ("", TabCommand::List),
            ("next", TabCommand::Next),
            ("prev", TabCommand::Previous),
            ("2", TabCommand::Select(2)),
            ("close 3", TabCommand::Close(Some(3))),
            ("close", TabCommand::Close(None)),
            ("restore", TabCommand::Restore),
            ("new", TabCommand::New),
        ] {
            assert_eq!(parse_tab_command(input), Ok(expected));
        }
        for input in [
            "0",
            "-1",
            "close 0",
            "next ignored",
            "unknown",
            "9999999999999999999999999",
        ] {
            assert!(parse_tab_command(input).is_err(), "{input}");
        }
    }
}
