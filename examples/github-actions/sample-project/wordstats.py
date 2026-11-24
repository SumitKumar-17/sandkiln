"""A tiny library standing in for the code under test in a pull request."""

from collections import Counter
import re


def top_words(text: str, n: int = 3) -> list[tuple[str, int]]:
    """Most frequent words, case-insensitive, ties broken alphabetically."""
    words = re.findall(r"[a-z']+", text.lower())
    counts = Counter(words)
    return sorted(counts.items(), key=lambda item: (-item[1], item[0]))[:n]
