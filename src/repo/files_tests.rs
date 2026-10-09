use super::{embed_hint, infer_mimetype, is_text_content};

#[test]
fn pgn_hints_pgn_directive() {
    assert_eq!(
        embed_hint("game.pgn", "application/octet-stream", 1),
        r#"<pgn id="1">"#
    );
}

#[test]
fn mermaid_hints_mermaid_directive() {
    assert_eq!(
        embed_hint("diagrams/flow.mmd", "text/plain", 2),
        r#"<mermaid id="2">"#
    );
}

#[test]
fn fen_hints_fen_directive() {
    assert_eq!(
        embed_hint("opening.fen", "application/x-chess-fen", 3),
        r#"<fen id="3">"#
    );
}

#[test]
fn json_hints_json_directive_with_query_placeholder() {
    assert_eq!(
        embed_hint("data/stats.json", "application/json", 4),
        r#"<json id="4" query=".">"#
    );
}

#[test]
fn image_mimetype_hints_image_directive() {
    assert_eq!(
        embed_hint("photo.jpg", "image/jpeg", 5),
        r#"<image id="5">"#
    );
}

#[test]
fn unknown_type_hints_file_directive() {
    assert_eq!(embed_hint("notes.txt", "text/plain", 6), r#"<file id="6">"#);
}

#[test]
fn extension_wins_over_generic_mimetype() {
    assert_eq!(
        embed_hint("game.pgn", "application/octet-stream", 7),
        r#"<pgn id="7">"#
    );
}

#[test]
fn infers_pgn_mimetype() {
    assert_eq!(infer_mimetype("game.pgn"), "application/x-chess-pgn");
}

#[test]
fn infers_mermaid_mimetype() {
    assert_eq!(infer_mimetype("diagrams/flow.mmd"), "text/vnd.mermaid");
}

#[test]
fn infers_fen_mimetype() {
    assert_eq!(infer_mimetype("opening.fen"), "text/plain");
}

#[test]
fn infers_known_extension_via_mime_guess() {
    assert_eq!(infer_mimetype("photo.jpg"), "image/jpeg");
}

#[test]
fn falls_back_to_octet_stream_for_unknown_extension() {
    assert_eq!(infer_mimetype("blob.bin"), "application/octet-stream");
}

#[test]
fn text_plain_is_text_content() {
    assert!(is_text_content("text/plain"));
}

#[test]
fn json_is_text_content() {
    assert!(is_text_content("application/json"));
}

#[test]
fn pgn_is_text_content() {
    assert!(is_text_content("application/x-chess-pgn"));
}

#[test]
fn image_is_not_text_content() {
    assert!(!is_text_content("image/png"));
}
