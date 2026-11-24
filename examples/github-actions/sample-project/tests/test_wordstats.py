import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from wordstats import top_words  # noqa: E402


class TopWords(unittest.TestCase):
    def test_counts_case_insensitively(self):
        self.assertEqual(top_words("a A b", 1), [("a", 2)])

    def test_ties_break_alphabetically(self):
        self.assertEqual(top_words("b a", 2), [("a", 1), ("b", 1)])

    def test_empty_text(self):
        self.assertEqual(top_words(""), [])


if __name__ == "__main__":
    unittest.main()
