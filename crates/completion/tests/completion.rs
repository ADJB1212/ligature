use ligature_completion::{Candidate, Edit, Kind, Provider, context, local, plan, rank, snippet};

fn candidate(label: &str, range: std::ops::Range<usize>, text: &str) -> Candidate {
    Candidate {
        label: label.into(),
        detail: String::new(),
        filter: label.into(),
        sort: label.into(),
        edit: Edit { range, text: text.into() },
        additional: Vec::new(),
        snippet: false,
        provider: Provider::Local,
    }
}

#[test]
fn contexts_complete_commands_environments_and_comma_separated_keys() {
    for (text, kind, prefix) in [
        ("\\sec", Kind::Command, "sec"),
        ("\\", Kind::Command, ""),
        ("\\begin{ali", Kind::Environment, "ali"),
        ("\\cref{first, sec:α", Kind::Label, "sec:α"),
        ("\\citep[see][p. 2]{first, Jo", Kind::Citation, "Jo"),
    ] {
        let context = context(text, text.len()).unwrap();
        assert_eq!(context.kind, kind);
        assert_eq!(context.prefix, prefix);
        assert_eq!(&text[context.range], prefix);
    }
    for text in ["% \\sec", "\\\\sec", "\\label{foo", "\\ref{foo}", "plain text"] {
        assert!(context(text, text.len()).is_none(), "{text}");
    }
    assert!(context("α", 1).is_none());
    assert!(context("a😀z", 6).is_none());
    assert!(context("e\u{301}", 3).is_none());
}

#[test]
fn local_keys_filter_and_server_candidates_take_precedence() {
    let context = context("\\ref{sec:", 9).unwrap();
    let mut candidates = local(&context, ["sec:one", "sec:two", "other"].into_iter(), std::iter::empty());
    let mut server = candidate("sec:one", context.range, "server insertion");
    server.provider = Provider::Server;
    candidates.push(server.clone());
    let ranked = rank(candidates, "sec:");
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0], server);
    let candidates = rank(
        vec![
            candidate("Section", 0..0, "a"),
            candidate("sectional", 0..0, "b"),
            candidate("section", 0..0, "c"),
        ],
        "section",
    );
    assert_eq!(candidates[0].label, "section");
    assert_eq!(candidates[1].label, "sectional");
}

#[test]
fn large_completion_sets_keep_the_best_200_in_order() {
    let mut candidates: Vec<_> = (0..1000)
        .rev()
        .map(|index| candidate(&format!("key:{index:04}"), 0..0, "value"))
        .collect();
    candidates.push(candidate("key:", 0..0, "exact"));
    candidates.push(candidate("KEY:0000", 0..0, "case insensitive"));
    candidates.push(candidate("unrelated", 0..0, "ignored"));
    let ranked = rank(candidates, "key:");
    assert_eq!(ranked.len(), 200);
    assert_eq!(ranked[0].label, "key:");
    for (index, candidate) in ranked[1..].iter().enumerate() {
        assert_eq!(candidate.label, format!("key:{index:04}"));
    }
    let names: Vec<_> = (0..1000).rev().map(|index| format!("key:{index:04}")).collect();
    let context = context("\\ref{key:", 9).unwrap();
    let local = local(&context, names.iter().map(String::as_str), std::iter::empty());
    assert_eq!(local.len(), 200);
    for (index, candidate) in local.iter().enumerate() {
        assert_eq!(candidate.label, format!("key:{index:04}"));
    }
}

#[test]
fn local_filtering_preserves_unicode_case_matching_and_duplicate_keys() {
    let context = context("\\ref{é", 7).unwrap();
    let candidates = local(&context, ["Éclair", "école", "other", "école"].into_iter(), std::iter::empty());
    assert_eq!(
        candidates.iter().map(|candidate| candidate.label.as_str()).collect::<Vec<_>>(),
        ["école", "Éclair"]
    );
}

#[test]
fn completion_validation_rejects_invalid_edits_and_snippets() {
    let text = "α😀tail";
    let mut item = candidate("valid", 6..10, "value");
    item.additional.push(Edit {
        range: 0..0,
        text: "prefix".into(),
    });
    assert!(ligature_completion::validate(text, &item).is_ok());
    item.additional.push(Edit {
        range: 7..8,
        text: "overlap".into(),
    });
    assert!(ligature_completion::validate(text, &item).is_err());
    item.additional.clear();
    for range in [1..2, 11..11, std::ops::Range { start: 8, end: 7 }] {
        item.edit.range = range;
        assert!(ligature_completion::validate(text, &item).is_err());
    }
    item.edit.range = 6..10;
    item.snippet = true;
    item.edit.text = "${1:unfinished".into();
    assert!(ligature_completion::validate(text, &item).is_err());
    item.edit.text = "${1:valid}$0".into();
    assert!(ligature_completion::validate(text, &item).is_ok());
    item.additional.push(Edit {
        range: 6..6,
        text: "first".into(),
    });
    item.additional.push(Edit {
        range: 6..6,
        text: "second".into(),
    });
    assert!(ligature_completion::validate(text, &item).is_err());
}

#[test]
fn snippets_expand_defaults_choices_nested_stops_and_final_caret() {
    let expansion = snippet("${2:β} ${1:${3:inner}} ${4|one,two|} $0").unwrap();
    let text = expansion.text;
    let stops = expansion.placeholders;
    assert_eq!(text, "β inner one ");
    assert_eq!(&text[stops[0].clone()], "inner");
    assert_eq!(&text[stops[1].clone()], "β");
    assert_eq!(&text[stops[2].clone()], "inner");
    assert_eq!(&text[stops[3].clone()], "one");
    assert_eq!(stops[4], text.len()..text.len());
    assert_eq!(snippet("${1:name} $1 \\$ \\\\ ${0}").unwrap().text, "name name $ \\ ");
    assert!(snippet("${1:unfinished").is_err());
    assert!(snippet("${1/regex/replace/}").is_err());
}

#[test]
fn additional_edits_preserve_surrounding_text_and_adjust_placeholders() {
    let text = "header\n\\sec tail";
    let mut candidate = candidate("section", 8..11, "section{${1:Title}}$0");
    candidate.snippet = true;
    candidate.additional.push(Edit {
        range: 0..0,
        text: "% added\n".into(),
    });
    let plan = plan(text, &candidate).unwrap();
    let mut result = text.into();
    String::replace_range(&mut result, plan.edit.range, &plan.edit.text);
    assert_eq!(result, "% added\nheader\n\\section{Title} tail");
    assert_eq!(&result[plan.placeholders[0].clone()], "Title");
    assert_eq!(plan.placeholders[1].start, result.find(" tail").unwrap());
    candidate.additional.push(Edit {
        range: 9..12,
        text: "invalid".into(),
    });
    assert!(ligature_completion::plan(text, &candidate).is_err());
}
