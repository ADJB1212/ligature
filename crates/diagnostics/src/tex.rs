use crate::{BuildId, Diagnostic, Provider, Severity, SourceRange};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

pub fn parse_tex(log: &str, working_directory: &Path, root_document: &Path, build: BuildId) -> Vec<Diagnostic> {
    let mut parser = Parser {
        directory: working_directory,
        root: resolve(working_directory, root_document),
        stack: Vec::new(),
        pending: None,
        diagnostics: Vec::new(),
        seen: BTreeSet::new(),
        build,
    };
    for line in log.lines() {
        parser.line(line.trim_end());
    }
    parser.flush();
    parser.diagnostics
}

struct Parser<'a> {
    directory: &'a Path,
    root: PathBuf,
    stack: Vec<Option<PathBuf>>,
    pending: Option<Diagnostic>,
    diagnostics: Vec<Diagnostic>,
    seen: BTreeSet<Diagnostic>,
    build: BuildId,
}

impl Parser<'_> {
    fn path(&self) -> PathBuf {
        self.stack.iter().rev().find_map(Clone::clone).unwrap_or_else(|| self.root.clone())
    }

    fn diagnostic(&self, severity: Severity, message: &str, path: Option<PathBuf>, range: Option<SourceRange>) -> Diagnostic {
        Diagnostic {
            severity,
            provider: Provider::Build(self.build),
            message: message.trim().into(),
            path,
            range,
        }
    }

    fn flush(&mut self) {
        if let Some(diagnostic) = self.pending.take()
            && self.seen.insert(diagnostic.clone())
        {
            self.diagnostics.push(diagnostic);
        }
    }

    fn line(&mut self, line: &str) {
        let trimmed = line.trim();
        if let Some((severity, rest)) = trimmed
            .strip_prefix("error: ")
            .map(|rest| (Severity::Error, rest))
            .or_else(|| trimmed.strip_prefix("warning: ").map(|rest| (Severity::Warning, rest)))
        {
            self.flush();
            let (path, range, message) = located_message(rest, self.directory);
            self.pending = Some(self.diagnostic(severity, message, path, range));
            self.flush();
            return;
        }
        if let Some(message) = trimmed.strip_prefix("! ") {
            self.flush();
            self.pending = Some(self.diagnostic(Severity::Error, message, Some(self.path()), None));
            return;
        }
        if let Some(rest) = trimmed.strip_prefix("l.")
            && let Some(number) = leading_number(rest)
        {
            if let Some(pending) = &mut self.pending {
                pending.range = Some(SourceRange::Lines { start: number, end: number });
            }
            self.flush();
            return;
        }
        if trimmed.starts_with("LaTeX Warning:")
            || (trimmed.starts_with("Package ") && trimmed.contains(" Warning:"))
            || trimmed.starts_with("Overfull ")
            || trimmed.starts_with("Underfull ")
        {
            self.flush();
            self.pending = Some(self.diagnostic(Severity::Warning, trimmed, Some(self.path()), line_range(trimmed)));
            if trimmed.ends_with('.') || trimmed.contains("at lines ") {
                self.flush();
            }
            return;
        }
        let (path, range, message) = located_message(trimmed, self.directory);
        if path.is_some() {
            self.flush();
            self.pending = Some(self.diagnostic(Severity::Error, message, path, range));
            self.flush();
            return;
        }
        if let Some(pending) = &mut self.pending {
            if trimmed.is_empty() {
                if pending.severity == Severity::Warning {
                    self.flush();
                }
                return;
            }
            if line.starts_with(char::is_whitespace)
                || trimmed.starts_with("(LaTeX)")
                || trimmed.starts_with("(Package ")
                || trimmed
                    .strip_prefix('(')
                    .and_then(|rest| rest.split_once(')'))
                    .is_some_and(|(name, rest)| {
                        !rest.trim().is_empty() && !name.is_empty() && name.chars().all(|character| character.is_alphanumeric() || character == '-')
                    })
                || (pending.severity == Severity::Warning && !trimmed.starts_with(['(', ')']) && !trimmed.starts_with("--- "))
            {
                pending.message.push(' ');
                pending.message.push_str(trimmed);
                if pending.severity == Severity::Warning {
                    pending.range = line_range(&pending.message);
                    if trimmed.ends_with('.') {
                        self.flush();
                    }
                }
                return;
            }
            self.flush();
        }
        if trimmed.starts_with("--- ") {
            self.stack.clear();
            return;
        }
        self.context(line);
    }

    fn context(&mut self, line: &str) {
        let mut remaining = line;
        while let Some(index) = remaining.find(['(', ')']) {
            let delimiter = remaining.as_bytes()[index];
            remaining = &remaining[index + 1..];
            if delimiter == b')' {
                self.stack.pop();
                continue;
            }
            let token = if let Some(quoted) = remaining.strip_prefix('"') {
                quoted.split_once('"').map(|(name, _)| name)
            } else {
                let candidate = remaining.split(['(', ')']).next().unwrap_or_default();
                [".tex", ".sty", ".cls", ".clo", ".ltx", ".aux", ".def", ".fd", ".cfg"]
                    .iter()
                    .filter_map(|extension| candidate.find(extension).map(|index| &candidate[..index + extension.len()]))
                    .min_by_key(|name| name.len())
                    .or_else(|| {
                        (!candidate.is_empty() && !candidate.contains([':', '\\', '=']) && candidate.chars().any(char::is_alphabetic))
                            .then_some(candidate)
                    })
            };
            let path = token.filter(|name| !name.is_empty()).map(|name| source_path(self.directory, name));
            self.stack.push(path);
            if let Some(token) = token {
                remaining = &remaining[token.len() + usize::from(remaining.starts_with('"'))..];
            }
        }
    }
}

fn leading_number(text: &str) -> Option<usize> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    text[..digits].parse().ok().filter(|number| *number > 0)
}

fn line_range(message: &str) -> Option<SourceRange> {
    if let Some((_, rest)) = message.split_once("at lines ") {
        let start = leading_number(rest)?;
        let end = rest.split_once("--").and_then(|(_, end)| leading_number(end)).unwrap_or(start);
        return (end >= start).then_some(SourceRange::Lines { start, end });
    }
    let rest = message.split_once("on input line ")?.1;
    let line = leading_number(rest)?;
    Some(SourceRange::Lines { start: line, end: line })
}

fn located_message<'a>(message: &'a str, directory: &Path) -> (Option<PathBuf>, Option<SourceRange>, &'a str) {
    for (index, _) in message.match_indices(':') {
        let rest = &message[index + 1..];
        let number = rest.strip_prefix('+').unwrap_or(rest);
        let digits = number.bytes().take_while(u8::is_ascii_digit).count();
        let Some(rest) = number[digits..].strip_prefix(':') else { continue };
        let Ok(line) = number[..digits].parse::<usize>() else { continue };
        if line == 0 || index == 0 {
            continue;
        }
        let path = message[..index].trim_matches('"');
        let range = SourceRange::Lines { start: line, end: line };
        return (Some(source_path(directory, path)), Some(range), rest.trim());
    }
    (None, None, message)
}

fn source_path(directory: &Path, name: &str) -> PathBuf {
    let path = Path::new(name);
    if path.extension().is_none() {
        resolve(directory, &path.with_extension("tex"))
    } else {
        resolve(directory, path)
    }
}

fn resolve(directory: &Path, path: &Path) -> PathBuf {
    let path = directory.join(path);
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            component => resolved.push(component),
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deeply_nested_context_preserves_the_innermost_source_location() {
        let log = format!("{}(child.tex\n! Undefined control sequence.\nl.4 \\missing\n", "(".repeat(10_000));
        let diagnostics = parse_tex(&log, Path::new("/project"), Path::new("main.tex"), BuildId(1));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].path.as_deref(), Some(Path::new("/project/child.tex")));
        assert_eq!(diagnostics[0].range, Some(SourceRange::Lines { start: 4, end: 4 }));
    }

    fn parse(log: &str) -> Vec<Diagnostic> {
        parse_tex(log, Path::new("/project"), Path::new("main.tex"), BuildId(9))
    }

    #[test]
    fn nested_files_multiline_errors_and_warnings_preserve_locations() {
        let diagnostics = parse(include_str!("../tests/fixtures/nested.log"));
        assert_eq!(diagnostics.len(), 5);
        assert_eq!(diagnostics[0].path, Some(PathBuf::from("/project/chapter one.tex")));
        assert_eq!(diagnostics[0].range, Some(SourceRange::Lines { start: 7, end: 7 }));
        assert!(diagnostics[0].message.contains("additional context"));
        assert_eq!(diagnostics[1].path, Some(PathBuf::from("/project/main.tex")));
        assert_eq!(diagnostics[1].range, Some(SourceRange::Lines { start: 12, end: 12 }));
        assert_eq!(diagnostics[2].range, Some(SourceRange::Lines { start: 13, end: 13 }));
        assert_eq!(diagnostics[3].range, Some(SourceRange::Lines { start: 20, end: 22 }));
        assert_eq!(diagnostics[4].range, None);
        assert!(diagnostics.iter().all(|diagnostic| diagnostic.provider == Provider::Build(BuildId(9))));
    }

    #[test]
    fn status_messages_do_not_assign_source_locations_to_resource_errors() {
        let diagnostics = parse(include_str!("../tests/fixtures/tectonic.txt"));
        assert_eq!(diagnostics.len(), 3);
        assert_eq!(diagnostics[0].path, Some(PathBuf::from("/project/chapters/α.tex")));
        assert_eq!(diagnostics[0].range, Some(SourceRange::Lines { start: 5, end: 5 }));
        assert_eq!(diagnostics[1].path, None);
        assert_eq!(diagnostics[1].range, None);
        assert_eq!(diagnostics[2].path, None);
    }

    #[test]
    fn tectonic_extensionless_include_context_resolves_tex_files() {
        let diagnostics = parse(
            "(main.tex\n(included\nLaTeX Warning: Reference `x' undefined on input line 1.\n)\n)\nerror: included:2: Undefined control sequence.",
        );
        assert_eq!(diagnostics.len(), 2);
        for diagnostic in diagnostics {
            assert_eq!(diagnostic.path, Some(PathBuf::from("/project/included.tex")));
        }
    }

    #[test]
    fn repeated_messages_missing_files_and_absent_lines_are_supported() {
        let diagnostics = parse("(./missing.tex\n! Undefined control sequence.\nl.2 \\bad\n)\n! Emergency stop.\n\n! Emergency stop.\n");
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].path, Some(PathBuf::from("/project/missing.tex")));
        assert_eq!(diagnostics[1].range, None);
    }

    #[test]
    fn wrapped_package_warnings_and_box_line_ranges_are_joined() {
        let diagnostics = parse(
            "Package hyperref Warning: Reference `x'\n(hyperref) undefined on input line 8.\nOverfull \\hbox (10pt too wide) in paragraph at lines\n10--12\n\n",
        );
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].range, Some(SourceRange::Lines { start: 8, end: 8 }));
        assert_eq!(diagnostics[1].range, Some(SourceRange::Lines { start: 10, end: 12 }));
    }
}
