"""The package imports with nothing installed and says its version."""

import re
import unittest

import ryucad


class Package(unittest.TestCase):
    def test_it_imports_and_has_a_version(self):
        self.assertRegex(ryucad.__version__, r"^\d+\.\d+\.\d+")

    def test_a_checkout_is_version_0_0_0(self):
        # Run from python/src, no distribution named ryucad is installed.
        from importlib import metadata

        try:
            metadata.version("ryucad")
        except metadata.PackageNotFoundError:
            self.assertEqual(ryucad.__version__, "0.0.0")
        else:
            self.assertTrue(re.match(r"^\d", ryucad.__version__))
