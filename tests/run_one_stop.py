import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
os.chdir(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests.test_indexer import RunIndexTests  # noqa: E402

if __name__ == "__main__":
    suite = unittest.TestSuite()
    suite.addTest(RunIndexTests("test_stop_file_stops_gracefully"))
    unittest.TextTestRunner(verbosity=2).run(suite)