//! The `video::`/`audio::` block macros: Ruby's `BlockMediaMacroRx` branch of
//! `next_block` and the sources `convert_video`/`convert_audio` build.
use adoctra_core::{parse, tokens, Block, BlockKind, Inline, Severity, Span, TokenKind};

fn blocks(source: &str) -> Vec<Block> {
    parse(source).document.into_flat_blocks()
}

/// The video fields of a block: target, src, poster and embedded.
fn video(block: &Block) -> (&str, &str, Option<&str>, bool) {
    match &block.kind {
        BlockKind::Video {
            target,
            src,
            poster,
            embedded,
        } => (target, src, poster.as_deref(), *embedded),
        other => panic!("not a video: {other:?}"),
    }
}

/// The audio fields of a block: target and src.
fn audio(block: &Block) -> (&str, &str) {
    match &block.kind {
        BlockKind::Audio { target, src } => (target, src),
        other => panic!("not an audio: {other:?}"),
    }
}

fn text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { value, .. } => value.clone(),
            Inline::Strong { children, .. } | Inline::CrossReference { children, .. } => {
                text(children)
            }
            other => panic!("unexpected inline {other:?}"),
        })
        .collect()
}

#[test]
fn video_macro_names_poster_width_height_and_resolves_its_sources() {
    let source = ":imagesdir: img\n\nvideo::my clip.mp4[poster.png,640,480,start=10,end=20,opts=\"autoplay,loop\"]\n";
    let blocks = blocks(source);
    let block = &blocks[0];
    let line = source.lines().nth(2).unwrap();
    assert_eq!(block.span, Span::new(17, 17 + line.len()));
    assert_eq!(
        video(block),
        (
            "my clip.mp4",
            "img/my%20clip.mp4#t=10,20",
            Some("img/poster.png"),
            false
        )
    );
    let named = &block.attrs.named;
    assert_eq!(
        (
            named["poster"].as_str(),
            named["width"].as_str(),
            named["height"].as_str()
        ),
        ("poster.png", "640", "480")
    );
    assert_eq!(block.attrs.options, ["autoplay", "loop"]);
}

#[test]
fn audio_macro_has_no_positional_names_and_drops_the_style() {
    let blocks = blocks("[Styled.role#myid]\naudio::s.ogg[first,end=3]\n");
    let block = &blocks[0];
    assert_eq!(audio(block), ("s.ogg", "s.ogg#t=,3"));
    assert_eq!(block.attrs.style, None);
    assert!(!block.attrs.named.contains_key("alt"));
    assert_eq!(block.id.as_deref(), Some("myid"));
    assert_eq!(block.roles, ["role"]);
}

#[test]
fn youtube_and_vimeo_posters_embed_players_with_the_scheme_at_conversion() {
    let blocks = blocks(
        "video::abc/PL1[youtube,opts=muted]\n\n:asset-uri-scheme:\n\n\
         [poster=vimeo]\nvideo::42/h[start=60]\n\n:asset-uri-scheme: http\n\nvideo::7[vimeo]\n",
    );
    assert_eq!(
        video(&blocks[0]),
        (
            "abc/PL1",
            "https://www.youtube.com/embed/abc?rel=0&mute=1&list=PL1",
            None,
            true
        )
    );
    assert_eq!(video(&blocks[1]).1, "//player.vimeo.com/video/42?h=h#at=60");
    assert_eq!(video(&blocks[2]).1, "http://player.vimeo.com/video/7");
}

#[test]
fn sources_follow_imagesdir_changes_in_the_body() {
    let blocks = blocks(
        ":imagesdir: a\n\nvideo::v.mp4[p.png]\n\n:imagesdir: b/\n\naudio::s.ogg[]\n\n\
         :imagesdir!:\n\nvideo::https://h.org/a b.mp4[https://h.org/p.png]\n",
    );
    assert_eq!(video(&blocks[0]).1, "a/v.mp4");
    assert_eq!(video(&blocks[0]).2, Some("a/p.png"));
    assert_eq!(audio(&blocks[1]).1, "b/s.ogg");
    assert_eq!(
        video(&blocks[2]),
        (
            "https://h.org/a b.mp4",
            "https://h.org/a%20b.mp4",
            Some("https://h.org/p.png"),
            false
        )
    );
}

#[test]
fn media_titles_are_not_captioned_but_label_references() {
    let blocks = blocks(
        ".Video *title*\n[[v1]]\nvideo::v.mp4[]\n\naudio::s.ogg[end=3,title=T2,id=a2]\n\n\
         :xrefstyle: full\n\n<<v1>> <<a2>>\n",
    );
    assert!(blocks[..2].iter().all(|b| b.caption.is_none()));
    assert_eq!(text(blocks[1].title.as_ref().unwrap()), "T2");
    let BlockKind::Paragraph { inlines } = &blocks[2].kind else {
        panic!("{:?}", blocks[2].kind);
    };
    assert_eq!(text(inlines), "Video title T2");
}

#[test]
fn drop_line_targets_drop_the_block_and_its_metadata() {
    let source = ":attribute-missing: drop-line\n\n[[gone]]\n.Dropped\nvideo::{nope}[]\n\npara\n";
    let result = parse(source);
    let blocks = result.document.into_flat_blocks();
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].id.is_none() && blocks[0].title.is_none());
    let info = &result.diagnostics[0];
    assert_eq!(info.severity, Severity::Info);
    let at = source.find("{nope}").unwrap();
    assert_eq!(info.span, Span::new(at, at + "{nope}".len()));
    // Under the default policy the reference stays in the target.
    assert_eq!(
        audio(&self::blocks("audio::{nope}s.ogg[]\n")[0]).1,
        "{nope}s.ogg"
    );
}

#[test]
fn list_continuations_verbatim_styles_and_paragraph_text() {
    let doc = parse("* item\n+\naudio::s.ogg[title=In list,id=x]\n").document;
    let BlockKind::List { items, .. } = &doc.blocks[0].kind else {
        panic!("{:?}", doc.blocks[0].kind);
    };
    let block = &items[0].blocks[0];
    assert_eq!(audio(block).0, "s.ogg");
    assert_eq!(block.id.as_deref(), Some("x"));
    assert_eq!(text(block.title.as_ref().unwrap()), "In list");
    assert_eq!(block.caption, None);

    let blocks =
        blocks("[verse]\naudio::s.ogg[]\n\n[literal]\nvideo::v.mp4[]\n\ntext\nvideo::v.mp4[]\n");
    assert!(matches!(blocks[0].kind, BlockKind::Verse { .. }));
    assert!(matches!(blocks[1].kind, BlockKind::Literal { .. }));
    assert!(matches!(blocks[2].kind, BlockKind::Paragraph { .. }));
}

#[test]
fn media_lines_are_block_macro_tokens() {
    let source = "video::v.mp4[]\n\naudio::s.ogg[]\n\ntext\nvideo::w.mp4[]\n";
    let macros: Vec<_> = tokens(source)
        .iter()
        .filter(|t| t.kind == TokenKind::BlockMacro)
        .map(|t| t.span)
        .collect();
    assert_eq!(macros, [Span::new(0, 14), Span::new(16, 30)]);
}
