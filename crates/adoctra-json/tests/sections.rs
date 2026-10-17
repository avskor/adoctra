use adoctra_core::{parse, Backend};
use adoctra_json::JsonBackend;

fn render(source: &str) -> String {
    String::from_utf8(JsonBackend.convert(&parse(source).document).unwrap()).unwrap()
}

#[test]
fn sections_and_preamble_serialize_as_nested_containers() {
    let json = render("= Doc\n\nPre\n\n== A\n\na\n\n=== B");
    assert!(
        json.contains(r#"{"name":"preamble","title":null,"blocks":[{"name":"paragraph""#),
        "{json}"
    );
    assert!(
        json.contains(
            r#"{"name":"section","level":1,"blocks":[{"name":"heading","level":1,"id":"_a""#
        ),
        "{json}"
    );
    assert!(
        json.contains(
            r#"{"name":"section","level":2,"blocks":[{"name":"heading","level":2,"id":"_b""#
        ),
        "{json}"
    );
    // A standalone preamble is unwrapped and a discrete heading stays a heading.
    let json = render("= Doc\n\nPre\n\n[discrete]\n== D");
    assert!(
        !json.contains("preamble") && !json.contains("section"),
        "{json}"
    );
}

#[test]
fn book_preamble_title_and_partintro_serialize() {
    let json = render("= B\n:doctype: book\n:preface-title: Fore\n\nPre\n\n= P\n\nIntro\n\n[abstract]\n--\nAbs\n--\n\n== C");
    assert!(
        json.contains(r#"{"name":"preamble","title":[{"name":"text","value":"Fore"}],"blocks":["#),
        "{json}"
    );
    // The part's content is wrapped in a partintro open block; the abstract
    // keeps its own style inside it.
    assert!(
        json.contains(r#"{"name":"open","id":null,"roles":[],"style":"partintro","attributes":{},"options":[],"title":null,"caption":null,"blocks":[{"name":"paragraph""#),
        "{json}"
    );
    assert!(json.contains(r#""style":"abstract""#), "{json}");
}

#[test]
fn toc_macro_and_normalized_toc_attributes_serialize() {
    let json = render("= D\n:toc: macro\n\n.Contents\n[[t]]\ntoc::[levels=1,role=side]\n\n== A");
    assert!(
        json.contains(r#"{"name":"toc","id":"t","roles":["side"],"style":null,"attributes":{"levels":"1"},"options":[],"title":[{"name":"text","value":"Contents"}],"caption":null}"#),
        "{json}"
    );
}

#[test]
fn images_serialize_target_src_alt_and_figure_caption() {
    let json =
        render(":imagesdir: img\n\n.Fig\n[[f]]\nimage::a b.png[(C) x,200]\n\nimage::my_pic.png[]");
    assert!(
        json.contains(r#"{"name":"image","target":"a b.png","src":"img/a%20b.png","alt":"&#169; x","id":"f","roles":[],"style":null,"attributes":{"alt":"(C) x","width":"200"},"options":[],"title":[{"name":"text","value":"Fig"}],"caption":"Figure 1. "}"#),
        "{json}"
    );
    assert!(
        json.contains(r#"{"name":"image","target":"my_pic.png","src":"img/my_pic.png","alt":"my pic","id":null"#),
        "{json}"
    );
}

#[test]
fn video_and_audio_serialize_target_src_poster_and_embedding() {
    let json = render(
        ":imagesdir: m\n\n.Clip\n[[v]]\nvideo::a.mp4[p.png,start=2]\n\nvideo::id[youtube]\n\naudio::s.ogg[role=r]",
    );
    assert!(
        json.contains(r#"{"name":"video","target":"a.mp4","src":"m/a.mp4#t=2","poster":"m/p.png","embedded":false,"id":"v","roles":[],"style":null,"attributes":{"poster":"p.png","start":"2"},"options":[],"title":[{"name":"text","value":"Clip"}],"caption":null}"#),
        "{json}"
    );
    assert!(
        json.contains(r#"{"name":"video","target":"id","src":"https://www.youtube.com/embed/id?rel=0","poster":null,"embedded":true,"id":null"#),
        "{json}"
    );
    assert!(
        json.contains(r#"{"name":"audio","target":"s.ogg","src":"m/s.ogg","id":null,"roles":["r"],"style":null,"attributes":{},"options":[],"title":null,"caption":null}"#),
        "{json}"
    );
}
