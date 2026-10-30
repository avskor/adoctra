import json
from pathlib import Path
import tempfile
import unittest

import visual
from visual import (STYLE, case_id, check, compare, load_manifest, measurement, no_worse, painted,
                    parse_color, signature, with_base)


def style(**changes):
    values = {"font-family": "serif", "font-size": "16px", "font-weight": "400", "font-style": "normal",
              "color": "#000000", "text-decoration-line": "none", "text-transform": "none",
              "letter-spacing": "normal", "font-variant-caps": "normal", "background-color": "#ffffff"}
    values.update({key.replace("_", "-"): value for key, value in changes.items()})
    return tuple(values[name] for name in STYLE)


def word(text, x=0, y=0, ctx="p", **changes):
    return (text, x, y, style(**changes), ctx)


class CompareTests(unittest.TestCase):
    def test_identical_words_have_no_differences(self):
        page = [word("a", 0, 0), word("b", 10, 0), word("c", 0, 20)]
        result = compare(page, page)
        self.assertEqual(result["style"], [])
        self.assertEqual((result["geometry"]["x"], result["geometry"]["dy"]), (0, 0))
        self.assertEqual((result["text"]["missing"], result["text"]["extra"]), (0, 0))

    def test_style_differences_are_grouped_with_an_example(self):
        ruby = [word("a", ctx="div.paragraph>p", font_family="serif"), word("b", 5, font_family="serif")]
        ours = [word("a", ctx="p", font_family="sans-serif"), word("b", 5, font_family="sans-serif")]
        [entry] = compare(ruby, ours)["style"]
        self.assertEqual(entry[:4], ["font-family", "serif", "sans-serif", 2])
        self.assertEqual(entry[4], {"text": "a", "ruby": "div.paragraph>p", "adoctra": "p"})

    def test_a_vertical_gap_counts_once_not_for_every_following_word(self):
        ruby = [word("a", 0, 0), word("b", 0, 20), word("c", 0, 40), word("d", 0, 60)]
        ours = [word("a", 0, 0), word("b", 0, 30), word("c", 0, 50), word("d", 0, 70)]
        geometry = compare(ruby, ours)["geometry"]
        self.assertEqual((geometry["x"], geometry["dy"]), (0, 1))
        self.assertEqual(geometry["examples"]["dy"], [[10, 1]])

    def test_horizontal_offsets_beyond_tolerance_are_counted(self):
        ruby = [word("a", 100.0), word("b", 120.0)]
        ours = [word("a", 100.8), word("b", 150.0)]
        geometry = compare(ruby, ours)["geometry"]
        self.assertEqual(geometry["x"], 1)
        self.assertEqual(geometry["examples"]["dx"], [[30, 1]])

    def test_missing_and_extra_words_are_text_differences(self):
        result = compare([word("Example"), word("1."), word("Title")], [word("Title"), word("Details")])
        self.assertEqual(result["text"]["missing"], 2)
        self.assertEqual(result["text"]["extra"], 1)
        self.assertEqual(result["text"]["examples"], {"missing": ["Example", "1."], "extra": ["Details"]})


class ColorTests(unittest.TestCase):
    def test_computed_colour_forms(self):
        self.assertEqual(parse_color("rgb(1, 2, 3)"), (1.0, 2.0, 3.0, 1.0))
        self.assertEqual(parse_color("rgba(0, 0, 0, 0.8)"), (0.0, 0.0, 0.0, 0.8))
        r, g, b, a = parse_color("color(srgb 0.5 1 0 / 0.25)")
        self.assertEqual((round(r, 1), g, b, a), (127.5, 255.0, 0.0, 0.25))
        with self.assertRaises(ValueError):
            parse_color("oklch(50% 0.1 30)")

    def test_text_and_nested_backgrounds_are_composited_over_white(self):
        self.assertEqual(painted("rgba(0, 0, 0, 0.8)", ""), ("#333333", "#ffffff"))
        # Inner semi-transparent layer over an opaque outer one.
        text, background = painted("rgb(0, 0, 0)", "rgba(255, 0, 0, 0.5) | rgb(0, 0, 255)")
        self.assertEqual((text, background), ("#000000", "#800080"))

    def test_equal_appearance_from_different_syntax_matches(self):
        self.assertEqual(painted("rgba(0, 0, 0, 0.8)", ""), painted("color(srgb 0.2 0.2 0.2)", ""))

    def test_driver_words_are_expanded_with_painted_colours(self):
        page = {"styles": [["serif", "16px", "400", "normal", "rgba(0, 0, 0, 0.8)", "none", "none", "normal",
                            "normal", "rgb(255, 255, 255)", "p"]],
                "words": [["a", 1, 2, 3, 4, 0]]}
        [(text, x, y, values, ctx)] = visual.words(page)
        self.assertEqual((text, x, y, ctx), ("a", 1, 2, "p"))
        self.assertEqual(values[STYLE.index("color")], "#333333")
        self.assertEqual(values[STYLE.index("background-color")], "#ffffff")


def record(**changes):
    page = {"width": 100, "height": 100, "styles": [["serif", "16px", "400", "normal", "rgb(0, 0, 0)", "none",
                                                     "none", "normal", "normal", "", "p"]],
            "words": [["a", 0, 0, 1, 1, 0]], "blocked": [], "errors": []}
    value = {"id": "c", "viewport": "desktop", "ruby": page, "adoctra": page,
             "pixels": {"different": 0, "total": 10000}}
    value.update(changes)
    return value


class BaselineTests(unittest.TestCase):
    def test_identical_pages_match(self):
        self.assertEqual(measurement(record())["status"], "match")

    def test_pixel_differences_above_the_limit_differ(self):
        self.assertEqual(measurement(record(pixels={"different": 50, "total": 10000}))["status"], "match")
        self.assertEqual(measurement(record(pixels={"different": 51, "total": 10000}))["status"], "differs")

    def test_driver_errors_are_errors_not_matches(self):
        result = measurement({"id": "c", "viewport": "desktop", "error": "boom"})
        self.assertEqual(signature(result), {"status": "error", "error": "boom"})

    def frozen(self, result, env="e"):
        return {"environment": env, "cases": {"c@desktop": signature(result)}}

    def test_drift_improvement_and_environment(self):
        bad = measurement(record(pixels={"different": 900, "total": 10000}))
        good = measurement(record())
        self.assertEqual(check(self.frozen(bad), self.frozen(bad)), [])
        self.assertEqual(check(self.frozen(good), self.frozen(bad)), [("c@desktop", "improved")])
        self.assertEqual(check(self.frozen(bad), self.frozen(good)), [("c@desktop", "drift")])
        self.assertEqual(check(self.frozen(bad, "other"), self.frozen(bad)), [("environment", "mismatch")])
        self.assertEqual(check({"environment": "e", "cases": {}}, self.frozen(bad)), [("c@desktop", "missing")])
        self.assertEqual(check(self.frozen(bad), {"environment": "e", "cases": {}}), [("c@desktop", "new")])

    def test_a_new_style_difference_is_drift_even_if_pixels_improve(self):
        before = {"status": "differs", "text": [0, 0], "style": [["color", "#000", "#111", 2]],
                  "geometry": [0, 0], "pixels": 5.0}
        after = dict(before, style=[["color", "#000", "#111", 2], ["font-size", "16px", "17px", 1]], pixels=1.0)
        self.assertFalse(no_worse(after, before))
        self.assertTrue(no_worse(dict(before, style=[["color", "#000", "#111", 1]]), before))


class CorpusTests(unittest.TestCase):
    def test_case_ids_and_base_injection(self):
        self.assertEqual(case_id(Path("tables/basic_spans.adoc")), "tables-basic-spans")
        html = with_base("<html><head>\n<meta charset=\"UTF-8\">", Path("/tmp/x"))
        self.assertTrue(html.startswith('<html><head><base href="file:///tmp/x/">'))
        with self.assertRaises(ValueError):
            with_base("<p>fragment</p>", Path("/tmp"))

    def test_committed_manifest_is_valid_and_matches_its_baseline(self):
        manifest = load_manifest(visual.DEFAULT_MANIFEST)
        baseline = json.loads((visual.CORPUS / "visual" / "report.json").read_text(encoding="utf-8"))
        expected = {f"{c['id']}@{v['name']}" for c in manifest["cases"] for v in manifest["viewports"]}
        self.assertEqual(set(baseline["cases"]), expected)
        self.assertEqual(baseline["profile"], visual.PROFILE)

    def test_embedded_page_wraps_the_fragment_with_its_stylesheet(self):
        page = visual.embedded_page("<p>x</p>\n", "p { color: red; }")
        self.assertIn("<style>\np { color: red; }\n</style>", page)
        self.assertIn("<body>\n<p>x</p>\n</body>", page)
        self.assertEqual(with_base(page, Path("/tmp")).count("<base "), 1)

    def test_manifest_validates_resources_and_embedded(self):
        manifest = json.loads(visual.DEFAULT_MANIFEST.read_text(encoding="utf-8"))
        source = "examples/sample.adoc"
        for case, valid in [
                ({"resources": {"example$a.adoc": source}}, True),
                ({"resources": {"main.adoc": source}}, False),
                ({"resources": {"../a.adoc": source}}, False),
                ({"resources": {"a.adoc": "missing.adoc"}}, False),
                ({"embedded": True}, True),
                ({"embedded": "yes"}, False)]:
            manifest["cases"] = [{"id": "x", "source": source, **case}]
            with tempfile.TemporaryDirectory() as scratch:
                path = Path(scratch) / "cases.json"
                path.write_text(json.dumps(manifest))
                if valid:
                    load_manifest(path)
                else:
                    with self.assertRaises(ValueError):
                        load_manifest(path)

    def test_vertical_shifts_name_their_first_words(self):
        ruby = [word("a", 0, 0, "p"), word("b", 0, 20, "pre")]
        ours = [word("a", 0, 0, "p"), word("b", 0, 31, "li>pre")]
        [shift] = compare(ruby, ours)["geometry"]["examples"]["shifted"]
        self.assertEqual(shift, {"text": "b", "dy": 11, "ruby": "pre", "adoctra": "li>pre"})

    def test_manifest_rejects_escaping_sources(self):
        manifest = json.loads(visual.DEFAULT_MANIFEST.read_text(encoding="utf-8"))
        manifest["cases"] = [{"id": "x", "source": "../outside.adoc"}]
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "cases.json"
            path.write_text(json.dumps(manifest))
            with self.assertRaises(ValueError):
                load_manifest(path)


if __name__ == "__main__":
    unittest.main()
