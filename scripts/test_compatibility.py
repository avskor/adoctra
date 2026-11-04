import json
import argparse
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import compatibility
from compatibility import (inputs_digest, case_commands, check, classify, html_classes, load_reference_cache, projection,
                           read_reference, run, store_reference_cache, validate_attributes, validate_resources,
                           verification_key)


class ProjectionTests(unittest.TestCase):
    def test_all_paragraphs_are_compared(self):
        self.assertNotEqual(projection("<p>one</p><p>two</p>"), projection("<p>one</p><p>wrong</p>"))

    def test_wrappers_and_entities_are_reconciled(self):
        self.assertEqual(projection('<div class="paragraph custom" id="p"><p>A &amp; B\nline</p></div>'),
                         projection('<p id="p" class="custom">A &amp; B line</p>'))

    def test_verbatim_whitespace_is_significant(self):
        ruby = '<div class="listingblock"><div class="content"><pre>a  b\n\tc</pre></div></div>'
        self.assertEqual(projection(ruby), projection('<pre class="listing">a  b\n\tc</pre>'))
        self.assertNotEqual(projection(ruby), projection('<pre class="listing">a b c</pre>'))

    def test_inline_markup_targets_and_unknown_attributes_survive(self):
        for a, b in [('<p><strong>x</strong></p>', '<p>x</p>'),
                     ('<p><a href="a">x</a></p>', '<p><a href="b">x</a></p>'),
                     ('<p data-custom="a">x</p>', '<p data-custom="b">x</p>')]:
            self.assertNotEqual(projection(a), projection(b))

    def test_list_kind_nesting_and_continuations_survive(self):
        ruby = '<div class="ulist"><ul>\n<li>\n<p>one</p>\n<ul><li><p>two</p></li></ul>\n<p>more</p></li></ul></div>'
        ours = '<ul><li>one\n<ul><li>two</li></ul><p>more</p></li></ul>'
        self.assertEqual(projection(ruby), projection(ours))
        self.assertNotEqual(projection(ours), projection(ours.replace('<ul>', '<ol>').replace('</ul>', '</ol>')))
        self.assertNotEqual(projection(ours), projection('<ul><li>one</li><li>two</li></ul><p>more</p>'))

    def test_quote_and_verse_layouts_are_reconciled(self):
        ruby = ('<div id="i" class="quoteblock r">\n<div class="title">T</div>\n<blockquote>\nPara <strong>b</strong>\n'
                '</blockquote>\n<div class="attribution">\n&#8212; A<br>\n<cite>W</cite>\n</div>\n</div>')
        ours = ('<figure class="quoteblock r" id="i">\n<div class="title">T</div>\n<blockquote>\n'
                'Para <strong>b</strong>\n</blockquote>\n<figcaption>&#8212; A<br>\n<cite>W</cite></figcaption>\n'
                '</figure>')
        self.assertEqual(projection(ruby), projection(ours))
        # Simple content (a quoted paragraph form) is not a compound paragraph.
        self.assertNotEqual(projection(ruby), projection(ours.replace('Para <strong>b</strong>',
                                                                      '<p>Para <strong>b</strong></p>')))
        verse = '<div class="verseblock"><pre class="content">a\n  b</pre><div class="attribution">\n&#8212; A\n</div></div>'
        self.assertEqual(projection(verse),
                         projection('<figure class="verseblock"><pre class="verse">a\n  b</pre>'
                                    '<figcaption>&#8212; A</figcaption></figure>'))
        # Attribution text, verse whitespace and multiple quoted paragraphs survive.
        self.assertNotEqual(projection(ours), projection(ours.replace('<cite>W</cite>', '<cite>X</cite>')))
        self.assertNotEqual(projection(verse), projection(verse.replace('a\n  b', 'a\nb')))
        self.assertNotEqual(projection('<blockquote><p>x</p><p>y</p></blockquote>'),
                            projection('<blockquote>x y</blockquote>'))

    def test_section_layouts_are_reconciled(self):
        ruby = ('<div id="preamble"><div class="sectionbody"><div class="paragraph"><p>P</p></div></div></div>'
                '<div class="sect1"><h2 id="_a">A</h2><div class="sectionbody"><div class="sect2">'
                '<h3 id="_b">B</h3></div></div></div><div class="sect1 r"><h2 id="_c">C</h2>'
                '<div class="sectionbody"><p>x</p></div></div>')
        ours = ('<section id="preamble"><p>P</p></section><section class="sect1"><h2 id="_a">A</h2>'
                '<section class="sect2"><h3 id="_b">B</h3></section></section>'
                '<section class="sect1 r"><h2 id="_c">C</h2><p>x</p></section>')
        self.assertEqual(projection(ruby), projection(ours))
        # The preamble and section roles remain observable.
        self.assertNotEqual(projection(ours), projection(ours.replace(' id="preamble"', '')))
        self.assertNotEqual(projection(ours), projection(ours.replace('sect1 r', 'sect1')))

    def test_toc_nav_is_ruby_toc_div(self):
        ruby = ('<div id="toc" class="toc"><div id="toctitle">T</div><ul class="sectlevel1">'
                '<li><a href="#_a">A</a></li></ul></div>')
        ours = ('<nav id="toc" class="toc"><div id="toctitle">T</div><ul class="sectlevel1">'
                '<li><a href="#_a">A</a></li></ul></nav>')
        self.assertEqual(projection(ruby), projection(ours))
        # Its id, class and title remain observable.
        self.assertNotEqual(projection(ours), projection(ours.replace('class="toc"', 'class="toc2"')))
        self.assertNotEqual(projection(ours), projection(ours.replace('>T<', '><')))

    def test_image_figure_is_ruby_imageblock(self):
        ruby = ('<div id="f" class="imageblock left"><div class="content"><img src="a.png" alt="A"></div>'
                '<div class="title">Figure 1. T</div></div>')
        ours = ('<figure id="f" class="imageblock left"><img src="a.png" alt="A">'
                '<figcaption class="title">Figure 1. T</figcaption></figure>')
        self.assertEqual(projection(ruby), projection(ours))
        # The image, its classes and the caption remain observable.
        self.assertNotEqual(projection(ours), projection(ours.replace('alt="A"', 'alt="B"')))
        self.assertNotEqual(projection(ours), projection(ours.replace('imageblock left', 'imageblock')))
        self.assertNotEqual(projection(ours), projection(ours.replace('Figure 1. ', '')))
        # A quote's figcaption is still its attribution.
        self.assertNotEqual(projection('<figcaption class="title">x</figcaption>'),
                            projection('<figcaption>x</figcaption>'))

    def test_sidebar_aside_is_ruby_sidebarblock(self):
        ruby = ('<div id="s" class="sidebarblock r"><div class="content"><div class="title">T</div>'
                '<div class="paragraph"><p>x</p></div></div></div>')
        ours = '<aside id="s" class="sidebarblock r"><header class="title">T</header><p>x</p></aside>'
        self.assertEqual(projection(ruby), projection(ours))
        # The aside's title header is Ruby's title div; a header elsewhere is not.
        self.assertNotEqual(projection('<div><header class="title">T</header></div>'),
                            projection('<div><div class="title">T</div></div>'))
        # Its id, roles, title and content remain observable.
        self.assertNotEqual(projection(ours), projection(ours.replace('sidebarblock r', 'sidebarblock')))
        self.assertNotEqual(projection(ours), projection(ours.replace('>T<', '><')))
        self.assertNotEqual(projection(ours), projection(ours.replace('<p>x</p>', '<p>y</p>')))
        # Other asides (admonitions) are not sidebars.
        self.assertNotEqual(projection(ruby), projection(ours.replace('sidebarblock r', 'admonition r')))

    def test_admonition_table_and_aside_are_reconciled(self):
        ruby = ('<div id="n" class="admonitionblock note r">\n<table>\n<tr>\n<td class="icon">\n'
                '<div class="title">Note</div>\n</td>\n<td class="content">\n<div class="title">T</div>\n'
                'Text <strong>b</strong>\n</td>\n</tr>\n</table>\n</div>')
        ours = ('<aside id="n" class="admonition r" data-type="note" aria-label="Note">\n'
                '<p class="admonition-title">Note</p>\n<div class="admonition-content">\n<div class="title">T</div>\n'
                'Text <strong>b</strong>\n</div>\n</aside>')
        self.assertEqual(projection(ruby), projection(ours))
        # The content wrapper is Ruby's content cell, not an extra block.
        self.assertNotEqual(projection(ruby), projection(ours.replace('admonition-content', 'openblock')))
        # Type, label, roles, id, title and body remain observable.
        for a, b in [('data-type="note"', 'data-type="tip"'), ('>Note</p>', '>Hint</p>'), ('admonition r', 'admonition'),
                     ('id="n"', 'id="m"'), ('>T<', '><'), ('Text <strong>', '<p>Text</p><strong>')]:
            self.assertNotEqual(projection(ruby), projection(ours.replace(a, b)), b)
        # Ruby's icon markup (icons mode) is not a text label.
        icons = ruby.replace('<div class="title">Note</div>', '<i class="fa icon-note" title="Note"></i>')
        self.assertNotEqual(projection(icons), projection(ours))
        # adoctra carries the same icon markup in its label.
        ours_icons = ours.replace('>Note</p>', '><i class="fa icon-note" title="Note"></i></p>')
        self.assertEqual(projection(icons), projection(ours_icons))
        image = icons.replace('<i class="fa icon-note" title="Note"></i>', '<img src="i/note.png" alt="Note">')
        self.assertNotEqual(projection(image), projection(ours_icons))

    def test_video_and_audio_figures_are_ruby_media_blocks(self):
        ruby = ('<div id="v" class="videoblock left"><div class="title">T</div><div class="content">'
                '<iframe src="https://y/e" frameborder="0" allowfullscreen></iframe></div></div>'
                '<div class="audioblock"><div class="content"><audio src="s.ogg" controls>x</audio></div></div>')
        ours = ('<figure id="v" class="videoblock left"><figcaption class="title">T</figcaption>'
                '<iframe src="https://y/e" allowfullscreen></iframe></figure>'
                '<figure class="audioblock"><audio src="s.ogg" controls>x</audio></figure>')
        self.assertEqual(projection(ruby), projection(ours))
        # Sources, booleans, classes and titles remain observable.
        self.assertNotEqual(projection(ours), projection(ours.replace(' allowfullscreen', '')))
        self.assertNotEqual(projection(ours), projection(ours.replace('s.ogg', 't.ogg')))
        self.assertNotEqual(projection(ours), projection(ours.replace('videoblock left', 'videoblock')))
        self.assertNotEqual(projection(ours), projection(ours.replace('>T<', '><')))
        # Only a zero frameborder is presentation.
        self.assertNotEqual(projection(ruby), projection(ruby.replace('frameborder="0"', 'frameborder="1"')))

    def test_description_lists_project_ruby_wrappers(self):
        ruby = ('<div id="d" class="dlist glossary r"><dl><dt>T</dt><dd><p>x</p></dd></dl></div>')
        ours = '<dl id="d" class="glossary r"><dt>T</dt><dd><p>x</p></dd></dl>'
        self.assertEqual(projection(ruby), projection(ours))
        ruby = '<div class="qlist qanda"><ol><li><p><em>Q</em></p><p>A</p></li></ol></div>'
        ours = '<ol class="qanda"><li><p><em>Q</em></p><p>A</p></li></ol>'
        self.assertEqual(projection(ruby), projection(ours))
        # Duplicate classes from a wrapper and its list merge once.
        self.assertEqual(projection('<div class="olist loweralpha"><ol class="loweralpha"><li><p>a</p></li></ol></div>'),
                         projection('<ol class="loweralpha"><li>a</li></ol>'))

    def test_callout_list_is_ruby_colist(self):
        ruby = '<div id="c" class="colist arabic r"><ol><li><p>one</p></li></ol></div>'
        ours = '<ol id="c" class="colist arabic r"><li>one</li></ol>'
        self.assertEqual(projection(ruby), projection(ours))
        self.assertNotEqual(projection(ours), projection(ours.replace(' r"', '"')))

    def test_ordered_list_roles_beside_the_default_style(self):
        ruby = '<div class="olist arabic dry"><ol class="arabic"><li><p>a</p></li></ol></div>'
        ours = '<ol class="arabic dry"><li>a</li></ol>'
        self.assertEqual(projection(ruby), projection(ours))
        self.assertNotEqual(projection(ours), projection('<ol class="arabic"><li>a</li></ol>'))
        self.assertNotEqual(projection(ours), projection('<ol class="loweralpha dry"><li>a</li></ol>'))

    def test_callout_list_with_icons_is_ruby_layout_table(self):
        ruby = ('<div id="c" class="colist arabic r"><table><tr><td><i class="conum" data-value="1"></i>'
                '<b>1</b></td><td>Describe <strong>it</strong></td></tr><tr><td><img src="2.png" alt="2">'
                '</td><td>Two</td></tr></table></div>')
        ours = ('<ol id="c" class="colist arabic icons r"><li><span class="colist-icon"><i class="conum" '
                'data-value="1"></i><b>1</b></span><div>Describe <strong>it</strong></div></li><li>'
                '<span class="colist-icon"><img src="2.png" alt="2"></span><div>Two</div></li></ol>')
        self.assertEqual(projection(ruby), projection(ours))
        # Icons, text, item count and roles stay significant.
        for changed in [ours.replace('alt="2"', 'alt="3"'), ours.replace('>Two<', '>Too<'),
                        ours.replace(' r"', '"'), ours.split('<li><span class="colist-icon"><img')[0] + '</ol>']:
            self.assertNotEqual(projection(changed), projection(ruby))
        # An icon is not text: the parts keep their roles.
        swapped = ours.replace('<span class="colist-icon"><img src="2.png" alt="2"></span><div>Two</div>',
                               '<span class="colist-icon">Two</span><div><img src="2.png" alt="2"></div>')
        self.assertNotEqual(projection(swapped), projection(ruby))

    def test_class_tokens_split_on_ascii_whitespace_only(self):
        # A no-break space stays inside a class name, like a Ruby role.
        self.assertEqual(html_classes("paragraph r\u00a0s\tt"), ["paragraph", "r\u00a0s", "t"])
        self.assertEqual(projection('<div class="paragraph r\u00a0s"><p>x</p></div>'),
                         projection('<p class="r\u00a0s">x</p>'))
        self.assertNotEqual(projection('<p class="r\u00a0s">x</p>'), projection('<p class="r s">x</p>'))

    def test_run_and_references_keep_a_lone_carriage_return(self):
        with patch("compatibility.subprocess.run",
                   return_value=subprocess.CompletedProcess([], 0, "<pre>\re</pre>".encode(), b"")):
            self.assertEqual(run(["x"], "s"), "<pre>\re</pre>")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "r.html"
            path.write_text("<pre>\re</pre>\n", encoding="utf-8", newline="")
            self.assertEqual(read_reference(path), "<pre>\re</pre>\n")

    def test_horizontal_description_list_is_ruby_layout_table(self):
        ruby = ('<div class="hdlist r"><table><colgroup><col style="width: 20%;"></colgroup><tr>'
                '<td class="hdlist1 strong">A<br>B</td><td class="hdlist2"><p>x</p></td></tr></table></div>')
        ours = ('<dl class="hdlist r" style="--adoctra-hdlist-labelwidth: 20%;"><div><dt class="strong">A</dt>'
                '<dt class="strong">B</dt><dd><p>x</p></dd></div></dl>')
        self.assertEqual(projection(ruby), projection(ours))
        self.assertNotEqual(projection(ours), projection(ours.replace('<dt class="strong">B</dt>', '')))
        self.assertNotEqual(projection(ours), projection(ours.replace('class="hdlist r"', 'class="hdlist"')))

    def test_full_document_head_is_outside_body_profile(self):
        self.assertEqual(projection('<html><head><style>p {color:red}</style></head><body><p>one</p></body></html>'),
                         projection('<p>one</p>'))

    def test_spaces_between_formatted_list_words_are_significant(self):
        ruby = '<ul><li>\n<p><strong>one</strong> <em>two</em></p>\n</li></ul>'
        ours = '<ul><li><strong>one</strong> <em>two</em></li></ul>'
        self.assertEqual(projection(ruby), projection(ours))
        self.assertNotEqual(projection(ours), projection(ours.replace('</strong> ', '</strong>')))

    def test_container_continuation_separates_item_text_from_block_layout(self):
        ruby = '<ul><li><p>one</p>\n<div class="openblock"><div class="content"><p>body</p></div></div></li></ul>'
        ours = '<ul><li>one\n<div class="openblock"><p>body</p></div></li></ul>'
        self.assertEqual(projection(ruby), projection(ours))
        self.assertNotEqual(projection(ours), projection(ours.replace('body', 'changed')))

    def test_thematic_break_separates_item_text(self):
        ruby = '<ul><li><p>item</p>\n<hr></li></ul>'
        ours = '<ul><li>item\n<hr>\n</li></ul>'
        self.assertEqual(projection(ruby), projection(ours))
        self.assertNotEqual(projection(ours), projection(ours.replace('<hr>', '')))

    def test_attached_admonition_separates_item_text(self):
        ruby = ('<ul><li><p>item</p>\n<div class="admonitionblock note"><table><tr><td class="icon">'
                '<div class="title">Note</div></td><td class="content">inside</td></tr></table></div></li></ul>')
        ours = ('<ul><li>item\n<aside class="admonition" data-type="note" aria-label="Note">'
                '<p class="admonition-title">Note</p><div class="admonition-content">inside</div></aside></li></ul>')
        self.assertEqual(projection(ruby), projection(ours))
        self.assertNotEqual(projection(ours), projection(ours.replace('item\n', 'item more\n')))

    def test_malformed_html_is_a_harness_error(self):
        for html in ['<p>unclosed', '<p>x</em>', '</div>']:
            with self.assertRaises(ValueError):
                projection(html)


class ExpectationTests(unittest.TestCase):
    def test_regressions_and_unexpected_fixes_fail(self):
        self.assertEqual(classify({'expect': 'pass'}, 'same', 'same'), 'PASS')
        self.assertEqual(classify({'expect': 'pass'}, 'wrong', 'right'), 'FAIL')
        self.assertEqual(classify({'expect': 'xfail'}, 'known', 'right', 'known'), 'XFAIL')
        self.assertEqual(classify({'expect': 'xfail'}, 'new regression', 'right', 'known'), 'FAIL')
        self.assertEqual(classify({'expect': 'xfail'}, 'right', 'right', 'known'), 'XPASS')


class ResourceFixtureTests(unittest.TestCase):
    def test_binary_resources_preserve_non_utf8_bytes_for_both_renderers(self):
        case = {"profile": "include-body-dom-v1", "source": "include::sub/a.adoc[encoding=UTF-16LE]",
                "resources": {}, "binary_resources": {"sub/a.adoc": "fffe16040d000a00"}}
        with case_commands(case, Path("adoctra")) as (reference, actual):
            root = Path(reference[-1])
            self.assertEqual(Path(actual[1]), root / "main.adoc")
            self.assertEqual((root / "sub/a.adoc").read_bytes(), bytes.fromhex("fffe16040d000a00"))
        self.assertFalse(root.exists())

    def test_binary_resource_hex_paths_and_collisions_are_validated(self):
        for binaries in [[], {"../escape": "ff"}, {"main.adoc": "ff"},
                         {"a": "f"}, {"a": "ff 00"}, {"a": "xy"}, {"a": 1}]:
            with self.assertRaises(ValueError):
                validate_resources({"profile": "include-body-dom-v1", "resources": {},
                                    "binary_resources": binaries})
        with self.assertRaises(ValueError):
            validate_resources({"profile": "include-body-dom-v1", "resources": {"a": "x"},
                                "binary_resources": {"a": "ff"}})
        with self.assertRaises(ValueError):
            validate_resources({"binary_resources": {"a": "ff"}})

    def test_invalid_resource_paths_and_profiles_are_rejected(self):
        for name in ["/absolute", "../escape", "a/../b", "a//b", "main.adoc", "a\\b", "x:y", ""]:
            with self.assertRaises(ValueError):
                validate_resources({"profile": "include-body-dom-v1", "resources": {name: "x"}})
        for case in [{"resources": {}}, {"profile": "include-body-dom-v1"}]:
            with self.assertRaises(ValueError):
                validate_resources(case)
        # An Antora resource id from another module is a plain file name.
        validate_resources({"profile": "include-body-dom-v1",
                            "resources": {"html-backend:example$my-document.adoc": "x"}})

    def test_both_renderers_use_the_same_isolated_utf8_tree(self):
        case = {"profile": "include-body-dom-v1", "source": "include::sub/я.adoc[]\r\n",
                "resources": {"sub/я.adoc": "Я\r\n"}}
        with case_commands(case, Path("adoctra")) as (reference, actual):
            root = Path(reference[-1])
            self.assertEqual(reference[-2], "--include-root")
            self.assertEqual(Path(actual[1]), root / "main.adoc")
            self.assertEqual((root / "sub/я.adoc").read_bytes(), "Я\r\n".encode())
        self.assertFalse(root.exists())

    def test_host_attributes_reach_both_renderers(self):
        case = {"profile": "include-body-dom-v1", "source": "include::a.adoc[]",
                "resources": {"a.adoc": "a"}, "attributes": {"max-include-depth": "1", "x": ""}}
        with case_commands(case, Path("adoctra")) as (reference, actual):
            self.assertEqual(reference[-2], "--include-root")
            for value in ["max-include-depth=1", "x="]:
                self.assertEqual(reference[reference.index(value) - 1], "--attribute")
                self.assertEqual(actual[actual.index(value) - 1], "-a")

    def test_invalid_host_attributes_are_rejected(self):
        for attributes in [[], {"": "x"}, {"-x": "x"}, {"a=b": "x"}, {"a": 1}]:
            with self.assertRaises(ValueError):
                validate_attributes({"id": "x", "attributes": attributes})


class ReferenceCacheTests(unittest.TestCase):
    CASE = {"id": "c", "category": "x", "expect": "pass", "source": "Hello"}

    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        (self.root / "reference").mkdir()
        (self.root / "reference" / "c.html").write_text("<p>Hello</p>\n", encoding="utf-8")
        corpus = patch("compatibility.CORPUS", self.root)
        corpus.start()
        self.addCleanup(corpus.stop)
        self.args = argparse.Namespace(verify_reference=True, update_reference=False,
                                       record_known_differences=False, binary=Path("adoctra"))

    def verify(self, cached, ruby="<p>Hello</p>\n", case=None, environment="env", diagnostics=None,
               ruby_diagnostics="[]", stderr=""):
        calls = []
        case = case or self.CASE
        if diagnostics is None:
            diagnostics = {"c": {"inputs": inputs_digest(case), "diagnostics": []}}

        def fake_run(command, source, stderr_wanted=False, **kwargs):
            calls.append(command[0])
            if command[0] == "ruby" and "--diagnostic-positions" in command:
                return '{"diagnostics": %s}' % ruby_diagnostics
            output = ruby if command[0] == "ruby" else "<p>Hello</p>"
            return (output, stderr) if kwargs.get("stderr") or stderr_wanted else output

        verified, hits = set(), []
        with patch("compatibility.run", side_effect=fake_run):
            result = check(case, self.args, environment, cached, verified, hits, diagnostics)
        return result, verified, hits, calls

    def test_an_unchanged_verified_reference_skips_ruby_but_still_compares_adoctra(self):
        result, verified, hits, calls = self.verify(set())
        # Ruby runs for the HTML and for the diagnostics reference.
        self.assertEqual((result["status"], hits, calls),
                         ("PASS", [], ["ruby", "ruby", str(Path("adoctra").resolve())]))
        result, again, hits, calls = self.verify(verified)
        self.assertEqual((result["status"], hits, again), ("PASS", ["c"], verified))
        self.assertNotIn("ruby", calls)

    def test_any_input_reference_or_environment_change_runs_ruby_again(self):
        _, verified, _, _ = self.verify(set())
        for change in ({"case": {**self.CASE, "attributes": {"a": "b"}}}, {"environment": "other"}):
            *_, calls = self.verify(verified, **change)
            self.assertIn("ruby", calls, change)
        (self.root / "reference" / "c.html").write_text("<p>Edited</p>\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "reference drift: c"):
            self.verify(verified)

    def test_diagnostics_are_a_separate_check_with_their_own_expectation(self):
        warning = {"severity": "warning", "message": "w", "location": {"file": "<stdin>", "line": 1}}
        self.args.verify_reference = False
        stored = {"c": {"inputs": inputs_digest(self.CASE), "diagnostics": [warning]}}
        result, *_ = self.verify(set(), diagnostics=stored, stderr="adoctra: -:1: warning: w\n")
        self.assertEqual((result["status"], result["diagnostics"]), ("PASS", "PASS"))
        result, *_ = self.verify(set(), diagnostics=stored, stderr="adoctra: -:2: warning: w\n")
        self.assertEqual((result["status"], result["diagnostics"]), ("FAIL", "FAIL"))
        with self.assertRaisesRegex(ValueError, "stale diagnostic reference: c"):
            self.verify(set(), diagnostics={"c": {"inputs": "old", "diagnostics": []}})
        # A known difference freezes adoctra's complete diagnostics.
        case = {**self.CASE, "diagnostics_expect": "xfail", "diagnostics_reason": "known"}
        stored = {"c": {"inputs": inputs_digest(case), "diagnostics": [warning]}}
        (self.root / "baseline").mkdir()
        (self.root / "baseline" / "c.diagnostics.json").write_text(json.dumps(
            [dict(warning, location={"file": "<stdin>", "line": 2})]), encoding="utf-8")
        result, *_ = self.verify(set(), case=case, diagnostics=stored, stderr="adoctra: -:2: warning: w\n")
        self.assertEqual((result["status"], result["diagnostics"]), ("PASS", "XFAIL"))
        result, *_ = self.verify(set(), case=case, diagnostics=stored, stderr="adoctra: -:1: warning: w\n")
        self.assertEqual((result["status"], result["diagnostics"]), ("XPASS", "XPASS"))

    def test_a_drifted_diagnostics_reference_fails_verification(self):
        with self.assertRaisesRegex(ValueError, "diagnostic reference drift: c"):
            self.verify(set(), ruby_diagnostics='[{"severity": "warning", "message": "w", "location": null}]')

    def test_without_an_environment_every_reference_runs_ruby(self):
        *_, calls = self.verify({"anything"}, environment=None)
        self.assertIn("ruby", calls)

    def test_the_environment_covers_only_the_scripts_the_reference_loads(self):
        import compatibility
        # The body mode loads no other script: the reference script requires
        # its helpers only in the modes this harness never passes.
        script = (compatibility.ROOT / "scripts/asciidoctor-reference.rb").read_text(encoding="utf-8")
        guarded = {"asciidoctor-semantics": "if semantics", "asciidoctor-structure": "if structure"}
        lines = script.split("\n")
        for index, line in enumerate(lines):
            if "require_relative" in line:
                name = line.split("'")[1]
                self.assertIn(name, guarded, line)
                self.assertEqual(lines[index - 1].strip(), guarded[name])
        with compatibility.case_commands(self.CASE, Path("adoctra")) as (reference, _):
            pass
        self.assertFalse({"--semantics", "--structure"} & set(reference))
        self.assertIn(str(compatibility.ROOT / "scripts" / compatibility.REFERENCE_SCRIPTS[0]), reference)
        with patch.object(compatibility, "ROOT", self.root):
            (self.root / "scripts").mkdir()
            for name in ("asciidoctor-reference.rb", "asciidoctor-structure.rb"):
                (self.root / "scripts" / name).write_text("a", encoding="utf-8")
            before = compatibility.ruby_environment()
            (self.root / "scripts/asciidoctor-structure.rb").write_text("b", encoding="utf-8")
            self.assertEqual(compatibility.ruby_environment(), before)
            (self.root / "scripts/asciidoctor-reference.rb").write_text("b", encoding="utf-8")
            self.assertNotEqual(compatibility.ruby_environment(), before)

    def test_the_key_ignores_the_temporary_include_root_only(self):
        case = {**self.CASE, "resources": {"a.adoc": "x"}}
        key = lambda command, reference="r": verification_key("env", case, command, reference)
        self.assertEqual(key(["ruby", "--include-root", "/tmp/a"]), key(["ruby", "--include-root", "/tmp/b"]))
        self.assertNotEqual(key(["ruby", "--showtitle"]), key(["ruby"]))
        self.assertNotEqual(key(["ruby"]), key(["ruby"], "changed"))
        self.assertNotEqual(key(["ruby"]), verification_key("env", {**case, "resources": {"a.adoc": "y"}},
                                                            ["ruby"], "r"))

    def test_the_cache_file_round_trips_and_tolerates_damage(self):
        path = self.root / "cache" / "compatibility.json"
        self.assertEqual(load_reference_cache(path), set())
        store_reference_cache({"b", "a"}, path)
        self.assertEqual(load_reference_cache(path), {"a", "b"})
        path.write_text("not json", encoding="utf-8")
        self.assertEqual(load_reference_cache(path), set())
        self.assertTrue(str(compatibility.REFERENCE_CACHE).startswith(str(compatibility.ROOT / "target")))


if __name__ == '__main__':
    unittest.main()
