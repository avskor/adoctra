import unittest

from compatibility import classify, projection


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


if __name__ == '__main__':
    unittest.main()
