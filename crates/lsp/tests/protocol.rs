use ligature_lsp::{Capabilities, Encoding, Position, SyncMode, byte_range, completions, diagnostics, hover, position, read_frame, uri, write_frame};
use serde_json::{Value, json};
use std::io::{self, BufReader, Cursor, Read};

struct Split(Cursor<Vec<u8>>);
impl Read for Split {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = buffer.len().min(1);
        self.0.read(&mut buffer[..count])
    }
}

#[test]
fn split_frames_and_multiple_messages_round_trip_unicode() {
    let messages = [json!({"id":1,"result":"α😀"}), json!({"method":"notify","params":{}})];
    let mut bytes = Vec::new();
    for message in &messages {
        write_frame(&mut bytes, message).unwrap();
    }
    let mut reader = BufReader::new(Split(Cursor::new(bytes)));
    for message in messages {
        assert_eq!(read_frame(&mut reader).unwrap(), Some(message));
    }
    assert_eq!(read_frame(&mut reader).unwrap(), None);
}

#[test]
fn malformed_oversized_and_truncated_frames_are_rejected() {
    for bytes in [
        b"Content-Length: 99999999\r\n\r\n".as_slice(),
        b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
        b"Content-Length: 4\r\n\r\n{}",
        b"Content-Length: no\r\n\r\n",
        b"Missing length\r\n\r\n",
    ] {
        assert!(read_frame(&mut Cursor::new(bytes)).is_err());
    }
}

#[test]
fn positions_use_negotiated_units_and_reject_partial_characters() {
    let text = "α😀z\r\nnext\rlast\n";
    for (encoding, character) in [(Encoding::Utf8, 6), (Encoding::Utf16, 3), (Encoding::Utf32, 2)] {
        let position = position(text, 6, encoding).unwrap();
        assert_eq!(position, Position { line: 0, character });
        assert_eq!(byte_range(text, &json!({"start":position,"end":position}), encoding), Some(6..6));
    }
    assert!(
        byte_range(
            text,
            &json!({"start":{"line":0,"character":2},"end":{"line":0,"character":3}}),
            Encoding::Utf16
        )
        .is_none()
    );
    assert!(position(text, 3, Encoding::Utf8).is_none());
    assert_eq!(position(text, text.len(), Encoding::Utf16), Some(Position { line: 3, character: 0 }));
    let path = std::path::Path::new("/project with spaces/α.tex");
    assert!(uri(path).unwrap().contains("%20"));
    assert!(uri(std::path::Path::new("relative.tex")).is_err());
    assert_eq!(
        ligature_lsp::completion_position("α😀\\be", 9, Encoding::Utf16),
        Some(Position { line: 0, character: 5 })
    );
}

#[test]
fn sync_modes_and_absent_features_follow_server_capabilities() {
    let none = Capabilities::parse(&json!({})).unwrap();
    assert_eq!(none.sync, SyncMode::None);
    assert!(!none.completion);
    assert!(!none.hover);
    assert!(none.change("old", "new").is_none());
    let full = Capabilities::parse(&json!({"textDocumentSync":1})).unwrap();
    assert!(full.open_close);
    assert_eq!(full.change("old", "new"), Some(json!({"text":"new"})));
    let incremental = Capabilities::parse(&json!({"positionEncoding":"utf-16","textDocumentSync":{"openClose":true,"change":2,"save":{"includeText":true}},"completionProvider":{},"hoverProvider":true})).unwrap();
    assert!(incremental.completion && incremental.hover && incremental.save && incremental.save_text);
    for (old, new) in [
        ("α😀z", "αβz"),
        ("line\r\nnext", "line\nnext"),
        ("line\r\nnext", "line\rnext"),
        ("", "α😀"),
        ("α😀", ""),
        ("first\r\nlast", "first\r\nlast!"),
    ] {
        let change = incremental.change(old, new).unwrap();
        let range = byte_range(old, &change["range"], incremental.encoding).unwrap();
        let mut applied = old.to_owned();
        applied.replace_range(range, change["text"].as_str().unwrap());
        assert_eq!(applied, new);
    }
}

#[test]
fn completion_edits_and_diagnostics_convert_from_utf16_to_bytes_and_scalars() {
    let text = "α😀\\sec";
    let range = json!({"start":{"line":0,"character":4},"end":{"line":0,"character":7}});
    let items = json!({"items":[{"label":"section","textEdit":{"range":range,"newText":"section{${1:Title}}"},"insertTextFormat":2},{"label":"bad","additionalTextEdits":[{"range":{"start":{"line":100,"character":0},"end":{"line":100,"character":0}},"newText":"x"}]}]});
    let candidates = completions(text, &items, 7..10, Encoding::Utf16);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].edit.range, 7..10);
    assert!(candidates[0].snippet);
    let diagnostics = diagnostics(
        text,
        std::path::Path::new("/p/a.tex"),
        &json!([{"range":range,"message":"problem","severity":2}]),
        Encoding::Utf16,
    );
    assert_eq!(diagnostics[0].range.as_ref().unwrap().byte_range(text), Some(7..10));
    assert_eq!(
        hover(&json!({"contents":["first",{"language":"latex","value":"second"}]})),
        Some("first\n\nsecond".into())
    );
    assert_eq!(hover(&Value::Null), None);
}

#[test]
fn batched_locations_preserve_unsorted_unicode_ranges_and_reject_invalid_items() {
    let text = "α😀z\r\n".repeat(2048);
    for (encoding, start, end) in [(Encoding::Utf8, 6, 7), (Encoding::Utf16, 3, 4), (Encoding::Utf32, 2, 3)] {
        let locations = [2047, 0, 1024, 3];
        let mut items: Vec<_> = locations
            .iter()
            .map(|line| json!({"label":format!("line {line}"),"message":"problem","textEdit":{"range":{"start":{"line":line,"character":start},"end":{"line":line,"character":end}},"newText":"replacement"},"range":{"start":{"line":line,"character":start},"end":{"line":line,"character":end}}}))
            .collect();
        items.push(json!({"label":"invalid","message":"invalid","textEdit":{"range":{"start":{"line":9000,"character":0},"end":{"line":9000,"character":0}},"newText":"x"},"range":{"start":{"line":9000,"character":0},"end":{"line":9000,"character":0}}}));
        let items = Value::Array(items);
        let candidates = completions(&text, &items, 0..0, encoding);
        let messages = diagnostics(&text, std::path::Path::new("/project/main.tex"), &items, encoding);
        assert_eq!(candidates.len(), locations.len());
        assert_eq!(messages.len(), locations.len());
        for ((candidate, diagnostic), line) in candidates.iter().zip(&messages).zip(locations) {
            let expected = line * 9 + 6..line * 9 + 7;
            assert_eq!(candidate.edit.range, expected);
            assert_eq!(diagnostic.range.as_ref().unwrap().byte_range(&text), Some(expected));
        }
    }
}
