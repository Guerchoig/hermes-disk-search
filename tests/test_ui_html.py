"""Валидность assets/ui.html: контент не должен оказаться после </html>.

РЕГРЕССИЯ: из-за битого слияния карточка «Расположение базы индексации»
и подпись про index.roots оказались после </body></html> — браузер
«переприклеивал» их вне контейнера .wrap и разметка разваливалась.
"""
import os
import re
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


class UiHtmlStructureTests(unittest.TestCase):
    def _html(self):
        root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        with open(os.path.join(root, "assets", "ui.html"), encoding="utf-8") as f:
            return f.read()

    def test_single_html_close_at_eof(self):
        html = self._html()
        self.assertEqual(html.count("</html>"), 1)
        self.assertTrue(html.rstrip().endswith("</html>"))

    def test_no_content_after_body(self):
        html = self._html()
        tail = html[html.rindex("</html>") + len("</html>"):].strip()
        self.assertEqual(tail, "")

    def test_required_ids_inside_body(self):
        html = self._html()
        body_close = html.rindex("</body>")
        for el in ("db-info", "db-new", "db-force", "trees-details",
                   "trees-box", "cfg", "events", "eta", "badges"):
            with self.subTest(el=el):
                self.assertLess(html.index('id="%s"' % el), body_close,
                                "id=%s должен быть внутри <body>" % el)

    def test_divs_balanced(self):
        opens = len(re.findall(r"<div\b", self._html()))
        closes = self._html().count("</div>")
        self.assertEqual(opens, closes, "дисбаланс <div>/</div>")


if __name__ == "__main__":
    unittest.main()
