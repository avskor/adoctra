import random
import unittest

import fuzz


class GenerationTests(unittest.TestCase):
    def test_generation_is_deterministic_per_seed(self):
        first = fuzz.generate(7, 40, [4])
        self.assertEqual(first, fuzz.generate(7, 40, [4]))
        self.assertNotEqual(first, fuzz.generate(8, 40, [4]))
        ids = [case_id for case_id, _ in first]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertEqual(len(first), 40 + len(fuzz.NESTING))
        # Slow families stop at their cap; the others nest to every depth.
        deep = [case_id for case_id, _ in fuzz.generate(7, 0, [4, 4096])]
        self.assertIn("nest-example-4096", deep)
        self.assertNotIn("nest-list-4096", deep)
        self.assertIn("nest-list-4", deep)

    def test_nesting_families_nest_to_the_requested_depth(self):
        example = fuzz.nesting("example", 3)
        self.assertEqual(example.splitlines(), ["====", "=====", "======", "core", "======", "=====", "===="])
        self.assertEqual(fuzz.nesting("list", 3), "* item 0\n** item 1\n*** item 2\n")
        self.assertEqual(fuzz.nesting("include-cond", 2), "ifdef::a0[]\nifdef::a1[]\ncore\nendif::[]\nendif::[]\n")
        for family in fuzz.NESTING:
            self.assertLess(len(fuzz.nesting(family, 4)), len(fuzz.nesting(family, 16)))

    def test_mutation_keeps_valid_text(self):
        rng = random.Random(1)
        pool = ["= T\n\nПривет *мир*\n\n----\ncode\n----\n", "* a\n** b\n"]
        for _ in range(200):
            text = fuzz.mutate(rng, rng.choice(pool), pool)
            text.encode("utf-8")  # Always whole characters.


class CompareTests(unittest.TestCase):
    def test_statuses(self):
        ruby = {"html": "<div class=\"paragraph\"><p>x</p></div>", "diagnostics": []}
        self.assertEqual(fuzz.compare(ruby, {"html": "<p>x</p>", "diagnostics": []}), ("PASS", None))
        self.assertEqual(fuzz.compare(ruby, {"html": "<p>y</p>", "diagnostics": []}), ("DIFF", "body"))
        warned = {"html": "<p>x</p>", "diagnostics": [{"severity": "warning", "message": "m"}]}
        self.assertEqual(fuzz.compare(ruby, warned), ("DIFF", "diagnostics"))
        self.assertEqual(fuzz.compare({"error": "E"}, warned), ("RUBY_ERROR", "E"))
        self.assertEqual(fuzz.compare(ruby, {"error": "exit 101"}), ("ADOCTRA_ERROR", "exit 101"))
        self.assertEqual(fuzz.compare(ruby, {"html": "<p>", "diagnostics": []})[0], "ADOCTRA_ERROR")


class MinimizeTests(unittest.TestCase):
    def test_minimizes_lines_then_characters_while_the_predicate_holds(self):
        source = "one\ntwo\nbad here\nthree\nfour"
        self.assertEqual(fuzz.minimize(source, lambda text: "bad" in text), "bad")
        # A predicate needing two lines keeps both.
        result = fuzz.minimize("a\nX\nb\nY\nc", lambda text: "X" in text and "Y" in text)
        self.assertEqual(result.replace("\n", ""), "XY")

    def test_respects_the_budget(self):
        calls = []

        def still(text):
            calls.append(text)
            return True

        fuzz.minimize("\n".join("x" * 50 for _ in range(50)), still, budget=10)
        self.assertLessEqual(len(calls), 10)


if __name__ == "__main__":
    unittest.main()
