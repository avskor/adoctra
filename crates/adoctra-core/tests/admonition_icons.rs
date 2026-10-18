//! Admonition icons, like Ruby's `convert_admonition`: the document `icons`
//! mode where the block converts and the image icon (`icon_uri`).

use adoctra_core::{parse, Block, BlockKind};

fn icons(source: &str) -> Vec<(Option<String>, String)> {
    parse(source)
        .document
        .flat_blocks()
        .into_iter()
        .filter_map(|block: &Block| match &block.kind {
            BlockKind::Admonition { icons, src, .. } => Some((icons.clone(), src.clone())),
            _ => None,
        })
        .collect()
}

fn icon(icons: Option<&str>, src: &str) -> (Option<String>, String) {
    (icons.map(str::to_owned), src.to_owned())
}

#[test]
fn without_icons_the_label_shows() {
    assert_eq!(icons("NOTE: x"), [icon(None, "")]);
}

#[test]
fn font_icons_need_no_image_unless_the_block_names_one() {
    assert_eq!(
        icons(":icons: font\n\nNOTE: a\n\n[TIP,icon=bulb]\nb"),
        [
            icon(Some("font"), ""),
            icon(Some("font"), "./images/icons/bulb.png")
        ]
    );
}

#[test]
fn image_icons_resolve_icon_uri_under_iconsdir() {
    // The default `iconsdir` derives from `imagesdir` before the header, and
    // a block `iconsdir` does not apply.
    let source = ":icons:\n:imagesdir: img\n\n[WARNING]\nc\n\n\
                  :iconsdir: ic\n:icontype: svg\n\n[CAUTION,icon=x.gif]\nd\n\n\
                  [IMPORTANT]\n====\ne\n====\n\n[NOTE,icon=n,iconsdir=own]\nf";
    assert_eq!(
        icons(source),
        [
            icon(Some(""), "./images/icons/warning.png"),
            icon(Some(""), "ic/x.gif"),
            icon(Some(""), "ic/important.svg"),
            icon(Some(""), "ic/n.svg"),
        ]
    );
}

#[test]
fn the_mode_is_read_where_the_block_converts() {
    // Body entries replay before the following block, like Ruby's converter
    // reading the document attributes.
    let source = "NOTE: a\n\n:icons: font\n\nNOTE: b\n\n:icons!:\n\nNOTE: c";
    assert_eq!(
        icons(source),
        [icon(None, ""), icon(Some("font"), ""), icon(None, "")]
    );
}
