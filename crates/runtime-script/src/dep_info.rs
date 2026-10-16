//! Bounded parser for the native Rustc Makefile dep-info subset we verify.
use crate::{ScriptError, freshness::MissReason};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
#[derive(Clone, Debug, Default)]
pub(crate) struct DepInfo {
    pub files: BTreeSet<PathBuf>,
    pub env: BTreeMap<OsString, Option<OsString>>,
}

pub(crate) fn read(
    path: &Path,
    base: &Path,
    cancelled: &AtomicBool,
) -> Result<Result<DepInfo, MissReason>, ScriptError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(ScriptError::Worker("compilation cancelled".into()));
    }
    let mut bytes = Vec::new();
    let result =
        fs::File::open(path).and_then(|file| file.take(1024 * 1024 + 1).read_to_end(&mut bytes));
    if result.is_err() {
        return Ok(Err(MissReason::UnreadableInput));
    }
    if bytes.len() > 1024 * 1024 {
        return Ok(Err(MissReason::LimitExceeded));
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(Err(MissReason::AmbiguousDepInfo));
    };
    Ok(parse(text, base, cfg!(windows)))
}

fn parse(text: &str, base: &Path, windows: bool) -> Result<DepInfo, MissReason> {
    let text = text.replace("\\\r\n", "").replace("\\\n", "");
    let mut result = DepInfo::default();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("# env-dep:") {
            let (name, value) = value
                .split_once('=')
                .map_or((value, None), |(name, value)| (name, Some(value)));
            if name.is_empty() || name.chars().any(char::is_control) {
                return Err(MissReason::AmbiguousDepInfo);
            }
            let value = value.map(unescape_env).transpose()?.map(OsString::from);
            if let Some(previous) = result.env.insert(name.into(), value.clone())
                && previous != value
            {
                return Err(MissReason::AmbiguousDepInfo);
            }
        } else if line.trim().is_empty() || line.starts_with('#') {
            continue;
        } else if let Some((target, dependencies)) = line.split_once(": ") {
            if target.is_empty() {
                return Err(MissReason::AmbiguousDepInfo);
            }
            for token in tokens(dependencies, windows)? {
                let path = PathBuf::from(token);
                result.files.insert(if path.is_absolute() {
                    path
                } else {
                    base.join(path)
                });
                if result.files.len() > 4096 {
                    return Err(MissReason::LimitExceeded);
                }
            }
        } else if !line.ends_with(':') {
            return Err(MissReason::AmbiguousDepInfo);
        }
    }
    if result.files.is_empty() {
        return Err(MissReason::AmbiguousDepInfo);
    }
    Ok(result)
}
fn tokens(text: &str, windows: bool) -> Result<Vec<String>, MissReason> {
    let mut chars = text.chars().peekable();
    let mut result = vec![];
    let mut token = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek().copied() {
                Some(' ' | '\t' | '#') => token.push(chars.next().unwrap()),
                Some('\\') if !windows => token.push(chars.next().unwrap()),
                Some(_) if windows => token.push(c),
                _ => return Err(MissReason::AmbiguousDepInfo),
            },
            '$' if chars.peek() == Some(&'$') => {
                chars.next();
                token.push('$');
            }
            '$' | '#' | '\0' => return Err(MissReason::AmbiguousDepInfo),
            ' ' | '\t' => {
                if !token.is_empty() {
                    result.push(std::mem::take(&mut token));
                }
            }
            c => token.push(c),
        }
    }
    if !token.is_empty() {
        result.push(token);
    }
    Ok(result)
}
fn unescape_env(text: &str) -> Result<String, MissReason> {
    let mut result = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        result.push(if c == '\\' {
            match chars.next() {
                Some('\\') => '\\',
                Some('n') => '\n',
                Some('r') => '\r',
                Some('t') => '\t',
                _ => return Err(MissReason::AmbiguousDepInfo),
            }
        } else {
            c
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_paths_continuations_and_environment_presence_are_distinct() {
        let parsed = parse("/out: src/main.rs path\\ with\\ space.rs \\\n inc\\#name.rs\nsrc/main.rs:\n# env-dep:ABSENT\n# env-dep:EMPTY=\n# env-dep:TEXT=a=b\\nc\\\\d\n", Path::new("/root"), false).unwrap();
        assert_eq!(parsed.files.len(), 3);
        assert!(
            parsed
                .files
                .contains(&PathBuf::from("/root/path with space.rs"))
        );
        assert_eq!(parsed.env[&OsString::from("ABSENT")], None);
        assert_eq!(parsed.env[&OsString::from("EMPTY")], Some(OsString::new()));
        assert_eq!(
            parsed.env[&OsString::from("TEXT")],
            Some(OsString::from("a=b\nc\\d"))
        );
        let windows = tokens(r"\\?\E:\sources\main.rs E:\path\ with\ space.rs", true).unwrap();
        assert_eq!(windows[0], r"\\?\E:\sources\main.rs");
        assert_eq!(windows[1], r"E:\path with space.rs");
    }
    #[test]
    fn ambiguous_or_empty_dep_info_does_not_claim_a_proof() {
        for text in [
            "",
            "unparsed line",
            "out: source$dynamic.rs",
            "out: a.rs\n# env-dep:X=\\q",
            "out: a.rs\n# env-dep:X\n# env-dep:X=",
        ] {
            assert!(parse(text, Path::new("."), false).is_err(), "{text}");
        }
    }
}
