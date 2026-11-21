use std::borrow::Cow;
use std::collections::HashSet;
use std::time::Duration;

use code_common::elapsed::format_duration;
use code_core::parse_command::ParsedCommand;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::history::compat::ExecAction;

use super::super::core::CommandOutput;
use super::super::exec::ParsedExecMetadata;
use super::super::formatting::output_lines;

use super::inline_scripts::format_inline_script_for_display;
use super::read_annotation::{coalesce_read_ranges_in_lines_local, parse_read_line_annotation};
use super::shell_display::{
    emphasize_shell_command_name,
    insert_line_breaks_after_double_ampersand,
    normalize_shell_command_display,
};
pub(crate) fn exec_render_parts_parsed_with_meta(
    parsed_commands: &[ParsedCommand],
    meta: &ParsedExecMetadata,
    output: Option<&CommandOutput>,
    stream_preview: Option<&CommandOutput>,
    elapsed_since_start: Option<Duration>,
    status_label: &str,
) -> (
    Vec<Line<'static>>,
    Vec<Line<'static>>,
    Option<Line<'static>>,
) {
    let action = meta.action;
    let s_text = crate::colors::style_text();
    let s_text_dim = crate::colors::style_text_dim();
    let ctx_path = meta.ctx_path.as_deref();
    let suppress_run_header = matches!(action, ExecAction::Run) && output.is_some();
    let mut pre: Vec<Line<'static>> = Vec::new();
    let mut running_status: Option<Line<'static>> = None;
    if !suppress_run_header {
        if output.is_none() { match action {
            ExecAction::Read => pre.push(Line::styled(
                "Read",
                s_text,
            )),
            ExecAction::Search => pre.push(Line::styled(
                "Search",
                s_text_dim,
            )),
            ExecAction::List => pre.push(Line::styled(
                "List",
                s_text,
            )),
            ExecAction::Run => {
                let mut message = match &ctx_path {
                    Some(p) => format!("{status_label}... in {p}"),
                    None => format!("{status_label}..."),
                };
                if let Some(elapsed) = elapsed_since_start {
                    message = format!("{message} ({})", format_duration(elapsed));
                }
                running_status = Some(running_status_line(message));
            }
        } } else {
            let done: Cow<'static, str> = match action {
                ExecAction::Read => "Read".into(),
                ExecAction::Search => "Search".into(),
                ExecAction::List => "List".into(),
                ExecAction::Run => match &ctx_path {
                    Some(p) => format!("Ran in {p}").into(),
                    None => "Ran".into(),
                },
            };
            if matches!(
                action,
                ExecAction::Read | ExecAction::Search | ExecAction::List
            ) {
                pre.push(Line::styled(
                    done,
                    s_text_dim,
                ));
            } else {
                pre.push(Line::styled(
                    done,
                    Style::default()
                        .fg(crate::colors::text_bright())
                        .add_modifier(Modifier::BOLD),
                ));
            }
        }
    }

    let search_paths = &meta.search_paths;
    // Compute output preview first to know whether to draw the downward corner.
    let show_stdout = matches!(action, ExecAction::Run);
    let display_output = output.or(stream_preview);
    let mut out = output_lines(display_output, !show_stdout, false);
    let mut any_content_emitted = false;
    // Determine allowed label(s) for this cell's primary action
    let expected_label: Option<&'static str> = match action {
        ExecAction::Read => Some("Read"),
        ExecAction::Search => Some("Search"),
        ExecAction::List => Some("List"),
        ExecAction::Run => None, // run: allow a set of labels
    };
    let use_content_connectors = !(matches!(action, ExecAction::Run) && output.is_none());

    for parsed in parsed_commands {
        let (label, content) = command_label_content(parsed, search_paths);
        // Enforce per-action grouping: only keep entries matching this cell's action.
        if let Some(exp) = expected_label {
            if label != exp {
                continue;
            }
        } else if !(label == "Run" || label == "Search") {
            // For generic "run" header, keep common run-like labels only.
            continue;
        }
        if label.is_empty() && content.is_empty() {
            continue;
        }
        for line_text in content.lines() {
            if line_text.is_empty() {
                continue;
            }
            let prefix = if !any_content_emitted {
                if suppress_run_header || !use_content_connectors {
                    ""
                } else {
                    "└ "
                }
            } else if suppress_run_header || !use_content_connectors {
                ""
            } else {
                "  "
            };
            let mut spans: Vec<Span<'static>> = Vec::new();
            if !prefix.is_empty() {
                spans.push(Span::styled(
                    prefix,
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
            match &*label {
                "Search" => {
                    let (terms_part, path_part): (&str, Option<&str>) = if let Some(idx) = line_text.rfind(" (in ") {
                        (&line_text[..idx], Some(&line_text[idx..]))
                    } else if let Some(idx) = line_text.rfind(" in ") {
                        let suffix = &line_text[idx + 1..];
                        if suffix.trim_end().ends_with('/') {
                            (&line_text[..idx], Some(&line_text[idx..]))
                        } else {
                            (line_text, None)
                        }
                    } else {
                        (line_text, None)
                    };
                    let chunks: Vec<&str> = if terms_part.contains(", ") {
                        terms_part.split(", ").collect()
                    } else {
                        vec![terms_part]
                    };
                    for (i, chunk) in chunks.iter().enumerate() {
                        if i > 0 {
                            spans.push(Span::styled(
                                ", ",
                                s_text_dim,
                            ));
                        }
                        if let Some((left, right)) = chunk.rsplit_once(" and ") {
                            if left.is_empty() {
                                spans.push(Span::styled(
                                    chunk.to_string(),
                                    s_text,
                                ));
                            } else {
                                spans.push(Span::styled(
                                    left.to_owned(),
                                    s_text,
                                ));
                                spans.push(Span::styled(
                                    " and ",
                                    s_text_dim,
                                ));
                                spans.push(Span::styled(
                                    right.to_owned(),
                                    s_text,
                                ));
                            }
                        } else {
                            spans.push(Span::styled(
                                chunk.to_string(),
                                s_text,
                            ));
                        }
                    }
                    if let Some(p) = path_part {
                        spans.push(Span::styled(
                            p.to_owned(),
                            s_text_dim,
                        ));
                    }
                }
                "Read" => {
                    if let Some(idx) = line_text.find(" (") {
                        let (fname, rest) = line_text.split_at(idx);
                        spans.push(Span::styled(
                            fname.to_owned(),
                            s_text,
                        ));
                        spans.push(Span::styled(
                            rest.to_owned(),
                            s_text_dim,
                        ));
                    } else {
                        spans.push(Span::styled(
                            line_text.to_owned(),
                            s_text,
                        ));
                    }
                }
                "List" => {
                    spans.push(Span::styled(
                        line_text.to_owned(),
                        s_text,
                    ));
                }
                _ => {
                    // Apply shell syntax highlighting to executed command lines.
                    // We highlight the single logical line as bash and append its spans inline.
                    let normalized = normalize_shell_command_display(line_text);
                    let display_line = insert_line_breaks_after_double_ampersand(&normalized);
                    let mut hl =
                        crate::syntax_highlight::highlight_code_block(&display_line, Some("bash"));
                    if let Some(mut first_line) = hl.pop() {
                        emphasize_shell_command_name(&mut first_line);
                        spans.extend(first_line.spans.into_iter());
                    } else {
                        spans.push(Span::styled(
                            display_line,
                            s_text,
                        ));
                    }
                }
            }
            pre.push(Line::from(spans));
            any_content_emitted = true;
        }
    }

    // If this is a List cell and nothing emitted (e.g., suppressed due to matching Search path),
    // still show a single contextual line so users can see where we listed.
    if matches!(action, ExecAction::List) && !any_content_emitted {
        let display_p = match &ctx_path {
            Some(p) if !p.is_empty() => {
                if p.ends_with('/') {
                    p.to_string()
                } else {
                    format!("{p}/")
                }
            }
            _ => "./".to_owned(),
        };
        pre.push(Line::from(vec![
            Span::styled("└ ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled(
                display_p,
                s_text,
            ),
        ]));
    }

    // Collapse adjacent Read ranges for the same file inside a single exec's preamble
    coalesce_read_ranges_in_lines_local(&mut pre);

    // Output: show stdout only for real run commands; errors always included
    // Collapse adjacent Read ranges for the same file inside a single exec's preamble
    coalesce_read_ranges_in_lines_local(&mut pre);

    if running_status.is_some()
        && let Some(last) = out.last() {
            let is_blank = last
                .spans
                .iter()
                .all(|sp| sp.content.as_ref().trim().is_empty());
            if is_blank {
                out.pop();
            }
        }

    (pre, out, running_status)
}

pub(crate) fn exec_render_parts_parsed(
    parsed_commands: &[ParsedCommand],
    output: Option<&CommandOutput>,
    stream_preview: Option<&CommandOutput>,
    elapsed_since_start: Option<Duration>,
    status_label: &str,
) -> (
    Vec<Line<'static>>,
    Vec<Line<'static>>,
    Option<Line<'static>>,
) {
    let meta = ParsedExecMetadata::from_commands(parsed_commands);
    exec_render_parts_parsed_with_meta(
        parsed_commands,
        &meta,
        output,
        stream_preview,
        elapsed_since_start,
        status_label,
    )
}

pub(crate) fn running_status_line(message: String) -> Line<'static> {
    Line::from(vec![
        Span::styled("└ ", crate::colors::style_border_dim()),
        Span::styled(message, crate::colors::style_text_dim()),
    ])
}

/// Unescape backslashes and balance unmatched `(` / `{` so UI text doesn't look broken.
fn prettify_search_term(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut iter = s.chars();
    while let Some(ch) = iter.next() {
        if ch == '\\' {
            if let Some(next) = iter.next() {
                out.push(next);
            } else {
                out.push('\\');
            }
        } else {
            out.push(ch);
        }
    }
    let opens_paren = out.matches('(').count();
    let closes_paren = out.matches(')').count();
    for _ in 0..opens_paren.saturating_sub(closes_paren) {
        out.push(')');
    }
    let opens_curly = out.matches('{').count();
    let closes_curly = out.matches('}').count();
    for _ in 0..opens_curly.saturating_sub(closes_curly) {
        out.push('}');
    }
    out
}

/// Format a pipe-separated query string into a readable comma/and list.
fn format_search_query(q: &str) -> String {
    let mut parts: Vec<String> = q
        .split('|')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(prettify_search_term)
        .collect();
    match parts.len() {
        0 => String::new(),
        1 => parts.remove(0),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => {
            let last = parts.last().cloned().unwrap_or_default();
            let head = &parts[..parts.len() - 1];
            format!("{} and {}", head.join(", "), last)
        }
    }
}

/// Map a single `ParsedCommand` to its display `(label, content)` pair.
///
/// Returns empty strings for entries that should be suppressed (e.g. directory
/// lines already covered by a search command).
fn command_label_content(
    parsed: &ParsedCommand,
    search_paths: &HashSet<String>,
) -> (Cow<'static, str>, Cow<'static, str>) {
    match parsed {
        ParsedCommand::Read { name, cmd, .. } => {
            let mut c = name.clone();
            if let Some(ann) = parse_read_line_annotation(cmd) {
                c = format!("{c} {ann}");
            }
            (Cow::Borrowed("Read"), Cow::Owned(c))
        }
        ParsedCommand::ListFiles { cmd: _, path } => match path {
            Some(p) => {
                if search_paths.contains(p) {
                    (Cow::Borrowed(""), Cow::Borrowed(""))
                } else {
                    let display_p = if p.ends_with('/') {
                        p.clone()
                    } else {
                        format!("{p}/")
                    };
                    (Cow::Borrowed("List"), Cow::Owned(display_p))
                }
            }
            None => (Cow::Borrowed("List"), Cow::Borrowed("./")),
        },
        ParsedCommand::Search { query, path, cmd } => {
            match (query.as_deref(), path.as_deref()) {
                (Some(q), Some(p)) => {
                    let display_p = if p.ends_with('/') {
                        p.to_owned()
                    } else {
                        format!("{p}/")
                    };
                    (
                        Cow::Borrowed("Search"),
                        Cow::Owned(format!("{} in {}", format_search_query(q), display_p)),
                    )
                }
                (Some(q), None) => (Cow::Borrowed("Search"), Cow::Owned(format_search_query(q))),
                (None, Some(p)) => {
                    let display_p = if p.ends_with('/') {
                        p.to_owned()
                    } else {
                        format!("{p}/")
                    };
                    (Cow::Borrowed("Search"), Cow::Owned(format!(" in {display_p}")))
                }
                (None, None) => (Cow::Borrowed("Search"), Cow::Owned(cmd.clone())),
            }
        }
        ParsedCommand::ReadCommand { cmd } => (Cow::Borrowed("Run"), Cow::Owned(cmd.clone())),
        ParsedCommand::Unknown { cmd } => {
            let t = cmd.trim();
            let lower = t.to_lowercase();
            if lower.starts_with("echo") && lower.contains("---") {
                (Cow::Borrowed(""), Cow::Borrowed(""))
            } else {
                (Cow::Borrowed("Run"), Cow::Owned(format_inline_script_for_display(cmd)))
            }
        }
    }
}
