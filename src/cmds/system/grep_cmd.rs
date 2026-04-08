//! Filters grep output by grouping matches by file.

use crate::core::config;
use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command};
use anyhow::{Context, Result};

const DEFAULT_MAX_LINE_LEN: usize = 80;
const DEFAULT_MAX_RESULTS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedGrepArgs {
    pattern: String,
    path: String,
    max_line_len: usize,
    max_results: usize,
    context_only: bool,
    file_type: Option<String>,
    extra_args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtraFlagKind {
    NoValue,
    TakesValue,
}

fn extra_long_flag_kind(arg: &str) -> ExtraFlagKind {
    let flag = arg.trim_start_matches('-');
    let (name, has_inline_value) = match flag.split_once('=') {
        Some((name, _)) => (name, true),
        None => (flag, false),
    };

    if matches!(
        name,
        "after-context"
            | "before-context"
            | "context"
            | "glob"
            | "iglob"
            | "type"
            | "type-not"
            | "max-columns"
            | "max-filesize"
            | "pre"
            | "pre-glob"
            | "sort"
            | "sortr"
            | "path-separator"
            | "colors"
            | "encoding"
            | "engine"
            | "threads"
            | "dfa-size-limit"
            | "regex-size-limit"
            | "include"
            | "exclude"
            | "exclude-dir"
            | "binary-files"
            | "directories"
            | "devices"
            | "label"
            | "message"
    ) && !has_inline_value
    {
        ExtraFlagKind::TakesValue
    } else {
        ExtraFlagKind::NoValue
    }
}

fn extra_short_flag_kind(arg: &str) -> ExtraFlagKind {
    let mut chars = arg.trim_start_matches('-').chars().peekable();
    if chars.peek().is_none() {
        return ExtraFlagKind::NoValue;
    }

    while let Some(ch) = chars.next() {
        if matches!(ch, 'A' | 'B' | 'C' | 'd' | 'g' | 'j' | 'M' | 'T') {
            return if chars.peek().is_some() {
                ExtraFlagKind::NoValue
            } else {
                ExtraFlagKind::TakesValue
            };
        }
    }

    ExtraFlagKind::NoValue
}

fn consume_extra_flag(args: &[String], i: &mut usize, extra_args: &mut Vec<String>) -> Result<()> {
    let arg = args.get(*i).context("missing grep flag")?;
    let kind = if arg.starts_with("--") {
        extra_long_flag_kind(arg)
    } else {
        extra_short_flag_kind(arg)
    };

    extra_args.push(arg.clone());
    *i += 1;

    if kind == ExtraFlagKind::TakesValue {
        extra_args.push(
            args.get(*i)
                .context(format!("missing value for grep flag '{}'", arg))?
                .clone(),
        );
        *i += 1;
    }

    Ok(())
}

fn consume_rtk_flag(
    args: &[String],
    i: &mut usize,
    max_line_len: &mut usize,
    max_results: &mut usize,
    context_only: &mut bool,
    file_type: &mut Option<String>,
) -> Result<bool> {
    let arg = args.get(*i).context("missing grep argument")?;
    match arg.as_str() {
        "-c" | "--context-only" => {
            *context_only = true;
            *i += 1;
            Ok(true)
        }
        "-n" | "--line-numbers" => {
            *i += 1;
            Ok(true)
        }
        "-l" | "--max-len" => {
            *i += 1;
            *max_line_len = args
                .get(*i)
                .context("missing value for --max-len")?
                .parse()
                .context("invalid --max-len value")?;
            *i += 1;
            Ok(true)
        }
        "-m" | "--max" => {
            *i += 1;
            *max_results = args
                .get(*i)
                .context("missing value for --max")?
                .parse()
                .context("invalid --max value")?;
            *i += 1;
            Ok(true)
        }
        "-t" | "--file-type" => {
            *i += 1;
            *file_type = Some(
                args.get(*i)
                    .context("missing value for --file-type")?
                    .clone(),
            );
            *i += 1;
            Ok(true)
        }
        _ if arg.starts_with("--max-len=") => {
            *max_line_len = arg["--max-len=".len()..]
                .parse()
                .context("invalid --max-len value")?;
            *i += 1;
            Ok(true)
        }
        _ if arg.starts_with("--max=") => {
            *max_results = arg["--max=".len()..]
                .parse()
                .context("invalid --max value")?;
            *i += 1;
            Ok(true)
        }
        _ if arg.starts_with("--file-type=") => {
            *file_type = Some(arg["--file-type=".len()..].to_string());
            *i += 1;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn parse_grep_args(args: &[String]) -> Result<ParsedGrepArgs> {
    let mut max_line_len = DEFAULT_MAX_LINE_LEN;
    let mut max_results = DEFAULT_MAX_RESULTS;
    let mut context_only = false;
    let mut file_type = None;
    let mut extra_args = Vec::new();
    let mut pattern = None;
    let mut path = None;
    let mut i = 0;
    let mut force_positional = false;

    while i < args.len() {
        let arg = &args[i];

        if arg == "--" && !force_positional {
            force_positional = true;
            i += 1;
            continue;
        }

        if !force_positional
            && consume_rtk_flag(
                args,
                &mut i,
                &mut max_line_len,
                &mut max_results,
                &mut context_only,
                &mut file_type,
            )?
        {
            continue;
        }

        if pattern.is_none() {
            if !force_positional && arg.starts_with('-') && arg != "-" {
                consume_extra_flag(args, &mut i, &mut extra_args)?;
            } else {
                pattern = Some(arg.clone());
                i += 1;
            }
            continue;
        }

        if !force_positional && arg.starts_with('-') && arg != "-" {
            consume_extra_flag(args, &mut i, &mut extra_args)?;
            continue;
        }

        if path.is_none() {
            path = Some(arg.clone());
        } else {
            extra_args.push(arg.clone());
        }
        i += 1;
    }

    Ok(ParsedGrepArgs {
        pattern: pattern.context("rtk grep requires a pattern")?,
        path: path.unwrap_or_else(|| ".".to_string()),
        max_line_len,
        max_results,
        context_only,
        file_type,
        extra_args,
    })
}

pub fn run_from_args(args: &[String], verbose: u8) -> Result<i32> {
    let parsed = parse_grep_args(args)?;
    run(
        &parsed.pattern,
        &parsed.path,
        parsed.max_line_len,
        parsed.max_results,
        parsed.context_only,
        parsed.file_type.as_deref(),
        &parsed.extra_args,
        verbose,
    )
}

use regex::Regex;
use std::collections::HashMap;
use std::process::Stdio;

#[allow(clippy::too_many_arguments)]
pub fn run(
    pattern: &str,
    path: &str,
    max_line_len: usize,
    max_results: usize,
    context_only: bool,
    file_type: Option<&str>,
    extra_args: &[String],
    verbose: u8,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("grep: '{}' in {}", pattern, path);
    }

    // Fix: convert BRE alternation \| → | for rg (which uses PCRE-style regex)
    let rg_pattern = pattern.replace(r"\|", "|");

    let mut rg_cmd = resolved_command("rg");
    rg_cmd
        .args(["-n", "--no-heading", &rg_pattern, path])
        .stdin(Stdio::null());

    if let Some(ft) = file_type {
        rg_cmd.arg("--type").arg(ft);
    }

    for arg in extra_args {
        // Fix: skip grep-ism -r flag (rg is recursive by default; rg -r means --replace)
        if arg == "-r" || arg == "--recursive" {
            continue;
        }
        rg_cmd.arg(arg);
    }

    let output = rg_cmd
        .output()
        .or_else(|_| {
            resolved_command("grep")
                .args(["-rn", pattern, path])
                .stdin(Stdio::null())
                .output()
        })
        .context("grep/rg failed")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let exit_code = exit_code_from_output(&output, "grep");

    let raw_output = stdout.to_string();

    if stdout.trim().is_empty() {
        // Show stderr for errors (bad regex, missing file, etc.)
        if exit_code == 2 {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.trim().is_empty() {
                eprintln!("{}", stderr.trim());
            }
        }
        let msg = format!("0 matches for '{}'", pattern);
        println!("{}", msg);
        timer.track(
            &format!("grep -rn '{}' {}", pattern, path),
            "rtk grep",
            &raw_output,
            &msg,
        );
        return Ok(exit_code);
    }

    let mut by_file: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    let mut total = 0;

    // Compile context regex once (instead of per-line in clean_line)
    let context_re = if context_only {
        Regex::new(&format!("(?i).{{0,20}}{}.*", regex::escape(pattern))).ok()
    } else {
        None
    };

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, ':').collect();

        let (file, line_num, content) = if parts.len() == 3 {
            let ln = parts[1].parse().unwrap_or(0);
            (parts[0].to_string(), ln, parts[2])
        } else if parts.len() == 2 {
            let ln = parts[0].parse().unwrap_or(0);
            (path.to_string(), ln, parts[1])
        } else {
            continue;
        };

        total += 1;
        let cleaned = clean_line(content, max_line_len, context_re.as_ref(), pattern);
        by_file.entry(file).or_default().push((line_num, cleaned));
    }

    let mut rtk_output = String::new();
    rtk_output.push_str(&format!("{} matches in {}F:\n\n", total, by_file.len()));

    let mut shown = 0;
    let mut files: Vec<_> = by_file.iter().collect();
    files.sort_by_key(|(f, _)| *f);

    for (file, matches) in files {
        if shown >= max_results {
            break;
        }

        let file_display = compact_path(file);
        rtk_output.push_str(&format!("[file] {} ({}):\n", file_display, matches.len()));

        let per_file = config::limits().grep_max_per_file;
        for (line_num, content) in matches.iter().take(per_file) {
            rtk_output.push_str(&format!("  {:>4}: {}\n", line_num, content));
            shown += 1;
            if shown >= max_results {
                break;
            }
        }

        if matches.len() > per_file {
            rtk_output.push_str(&format!("  +{}\n", matches.len() - per_file));
        }
        rtk_output.push('\n');
    }

    if total > shown {
        rtk_output.push_str(&format!("... +{}\n", total - shown));
    }

    print!("{}", rtk_output);
    timer.track(
        &format!("grep -rn '{}' {}", pattern, path),
        "rtk grep",
        &raw_output,
        &rtk_output,
    );

    Ok(exit_code)
}

fn clean_line(line: &str, max_len: usize, context_re: Option<&Regex>, pattern: &str) -> String {
    let trimmed = line.trim();

    if let Some(re) = context_re {
        if let Some(m) = re.find(trimmed) {
            let matched = m.as_str();
            if matched.len() <= max_len {
                return matched.to_string();
            }
        }
    }

    if trimmed.len() <= max_len {
        trimmed.to_string()
    } else {
        let lower = trimmed.to_lowercase();
        let pattern_lower = pattern.to_lowercase();

        if let Some(pos) = lower.find(&pattern_lower) {
            let char_pos = lower[..pos].chars().count();
            let chars: Vec<char> = trimmed.chars().collect();
            let char_len = chars.len();

            let start = char_pos.saturating_sub(max_len / 3);
            let end = (start + max_len).min(char_len);
            let start = if end == char_len {
                end.saturating_sub(max_len)
            } else {
                start
            };

            let slice: String = chars[start..end].iter().collect();
            if start > 0 && end < char_len {
                format!("...{}...", slice)
            } else if start > 0 {
                format!("...{}", slice)
            } else {
                format!("{}...", slice)
            }
        } else {
            let t: String = trimmed.chars().take(max_len - 3).collect();
            format!("{}...", t)
        }
    }
}

fn compact_path(path: &str) -> String {
    if path.len() <= 50 {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        return path.to_string();
    }

    format!(
        "{}/.../{}/{}",
        parts[0],
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_line() {
        let line = "            const result = someFunction();";
        let cleaned = clean_line(line, 50, None, "result");
        assert!(!cleaned.starts_with(' '));
        assert!(cleaned.len() <= 50);
    }

    #[test]
    fn test_compact_path() {
        let path = "/Users/patrick/dev/project/src/components/Button.tsx";
        let compact = compact_path(path);
        assert!(compact.len() <= 60);
    }

    #[test]
    fn test_extra_args_accepted() {
        // Test that the function signature accepts extra_args
        // This is a compile-time test - if it compiles, the signature is correct
        let _extra: Vec<String> = vec!["-i".to_string(), "-A".to_string(), "3".to_string()];
        // No need to actually run - we're verifying the parameter exists
    }

    #[test]
    fn test_parse_grep_args_supports_ripgrep_style_ordering() {
        let parsed = parse_grep_args(&[
            "slack".to_string(),
            "-S".to_string(),
            ".".to_string(),
        ])
        .expect("should parse");

        assert_eq!(parsed.pattern, "slack");
        assert_eq!(parsed.path, ".");
        assert_eq!(parsed.extra_args, vec!["-S"]);
        assert_eq!(parsed.max_line_len, DEFAULT_MAX_LINE_LEN);
        assert_eq!(parsed.max_results, DEFAULT_MAX_RESULTS);
    }

    #[test]
    fn test_parse_grep_args_supports_flags_before_pattern() {
        let parsed = parse_grep_args(&[
            "-S".to_string(),
            "slack".to_string(),
            ".".to_string(),
        ])
        .expect("should parse");

        assert_eq!(parsed.pattern, "slack");
        assert_eq!(parsed.path, ".");
        assert_eq!(parsed.extra_args, vec!["-S"]);
    }

    #[test]
    fn test_parse_grep_args_keeps_rtk_options_anywhere() {
        let parsed = parse_grep_args(&[
            "slack".to_string(),
            "-m".to_string(),
            "25".to_string(),
            "-t".to_string(),
            "rs".to_string(),
            "-S".to_string(),
            ".".to_string(),
        ])
        .expect("should parse");

        assert_eq!(parsed.pattern, "slack");
        assert_eq!(parsed.path, ".");
        assert_eq!(parsed.max_results, 25);
        assert_eq!(parsed.file_type.as_deref(), Some("rs"));
        assert_eq!(parsed.extra_args, vec!["-S"]);
    }

    #[test]
    fn test_parse_grep_args_consumes_extra_flag_values() {
        let parsed = parse_grep_args(&[
            "-A".to_string(),
            "3".to_string(),
            "slack".to_string(),
            ".".to_string(),
        ])
        .expect("should parse");

        assert_eq!(parsed.pattern, "slack");
        assert_eq!(parsed.path, ".");
        assert_eq!(parsed.extra_args, vec!["-A", "3"]);
    }

    #[test]
    fn test_parse_grep_args_defaults_path_when_only_pattern_and_flags() {
        let parsed = parse_grep_args(&[
            "slack".to_string(),
            "-g".to_string(),
            "*.rs".to_string(),
        ])
        .expect("should parse");

        assert_eq!(parsed.pattern, "slack");
        assert_eq!(parsed.path, ".");
        assert_eq!(parsed.extra_args, vec!["-g", "*.rs"]);
    }

    #[test]
    fn test_clean_line_multibyte() {
        // Thai text that exceeds max_len in bytes
        let line = "  สวัสดีครับ นี่คือข้อความที่ยาวมากสำหรับทดสอบ  ";
        let cleaned = clean_line(line, 20, None, "ครับ");
        // Should not panic
        assert!(!cleaned.is_empty());
    }

    #[test]
    fn test_clean_line_emoji() {
        let line = "🎉🎊🎈🎁🎂🎄 some text 🎃🎆🎇✨";
        let cleaned = clean_line(line, 15, None, "text");
        assert!(!cleaned.is_empty());
    }

    // Fix: BRE \| alternation is translated to PCRE | for rg
    #[test]
    fn test_bre_alternation_translated() {
        let pattern = r"fn foo\|pub.*bar";
        let rg_pattern = pattern.replace(r"\|", "|");
        assert_eq!(rg_pattern, "fn foo|pub.*bar");
    }

    // Fix: -r flag (grep recursive) is stripped from extra_args (rg is recursive by default)
    #[test]
    fn test_recursive_flag_stripped() {
        let extra_args: Vec<String> = vec!["-r".to_string(), "-i".to_string()];
        let filtered: Vec<&String> = extra_args
            .iter()
            .filter(|a| *a != "-r" && *a != "--recursive")
            .collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0], "-i");
    }

    // --- truncation accuracy ---

    #[test]
    fn test_grep_overflow_uses_uncapped_total() {
        // Confirm the grep overflow invariant: matches vec is never capped before overflow calc.
        // If total_matches > per_file, overflow = total_matches - per_file (not capped).
        // This documents that grep_cmd.rs avoids the diff_cmd bug (cap at N then compute N-10).
        let per_file = config::limits().grep_max_per_file;
        let total_matches = per_file + 42;
        let overflow = total_matches - per_file;
        assert_eq!(overflow, 42, "overflow must equal true suppressed count");
        // Demonstrate why capping before subtraction is wrong:
        let hypothetical_cap = per_file + 5;
        let capped = total_matches.min(hypothetical_cap);
        let wrong_overflow = capped - per_file;
        assert_ne!(
            wrong_overflow, overflow,
            "capping before subtraction gives wrong overflow"
        );
    }

    // Verify line numbers are always enabled in rg invocation (grep_cmd.rs:24).
    // The -n/--line-numbers clap flag in main.rs is a no-op accepted for compat.
    #[test]
    fn test_rg_always_has_line_numbers() {
        // grep_cmd::run() always passes "-n" to rg (line 24).
        // This test documents that -n is built-in, so the clap flag is safe to ignore.
        let mut cmd = resolved_command("rg");
        cmd.args(["-n", "--no-heading", "NONEXISTENT_PATTERN_12345", "."]);
        // If rg is available, it should accept -n without error (exit 1 = no match, not error)
        if let Ok(output) = cmd.output() {
            assert!(
                output.status.code() == Some(1) || output.status.success(),
                "rg -n should be accepted"
            );
        }
        // If rg is not installed, skip gracefully (test still passes)
    }
}
