use ligature_completion::{Kind, context};

#[test]
fn argument_context_uses_current_line_and_absolute_offsets() {
    let prefix = "ordinary text without braces\n".repeat(10_000);
    for (suffix, kind, expected) in [
        ("\\ref{sec:猫", Kind::Label, "sec:猫"),
        ("\\cite{first, second", Kind::Citation, "second"),
        ("\\begin{ali", Kind::Environment, "ali"),
        ("\\ref\n{sec", Kind::Label, "sec"),
    ] {
        let text = format!("{prefix}{suffix}");
        let found = context(&text, text.len()).unwrap();
        assert_eq!(found.kind, kind);
        assert_eq!(found.prefix, expected);
        assert_eq!(&text[found.range], expected);
    }
    for suffix in ["plain text", "\\ref{old\ncurrent", "\\ref{old}\ncurrent"] {
        let text = format!("{prefix}{suffix}");
        assert!(context(&text, text.len()).is_none());
    }
}
