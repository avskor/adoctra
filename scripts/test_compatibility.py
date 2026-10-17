import unittest
from pathlib import Path

from compatibility import case_commands, classify, projection, validate_resources


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
                '<p>Para <strong>b</strong></p>\n</blockquote>\n<figcaption>&#8212; A<br>\n<cite>W</cite></figcaption>\n'
                '</figure>')
        self.assertEqual(projection(ruby), projection(ours))
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
    def test_invalid_resource_paths_and_profiles_are_rejected(self):
        for name in ["/absolute", "../escape", "a/../b", "a//b", "main.adoc", "a\\b", "x:y", ""]:
            with self.assertRaises(ValueError):
                validate_resources({"profile": "include-body-dom-v1", "resources": {name: "x"}})
        for case in [{"resources": {}}, {"profile": "include-body-dom-v1"}]:
            with self.assertRaises(ValueError):
                validate_resources(case)

    def test_both_renderers_use_the_same_isolated_utf8_tree(self):
        case = {"profile": "include-body-dom-v1", "source": "include::sub/я.adoc[]\r\n",
                "resources": {"sub/я.adoc": "Я\r\n"}}
        with case_commands(case, Path("adoctra")) as (reference, actual):
            root = Path(reference[-1])
            self.assertEqual(reference[-2], "--include-root")
            self.assertEqual(Path(actual[1]), root / "main.adoc")
            self.assertEqual((root / "sub/я.adoc").read_bytes(), "Я\r\n".encode())
        self.assertFalse(root.exists())


if __name__ == '__main__':
    unittest.main()
