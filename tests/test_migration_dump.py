import json
import unittest

from tests.test_standalone import compile_fixture


class MigrationDump(unittest.TestCase):
    def test_dump_canonical_outputs(self):
        outputs = {}
        for fixture in ("menagerie", "library"):
            compilation, _ = compile_fixture(fixture)
            outputs[fixture] = dict(compilation.files)
        print("RUST_MIGRATION_ORACLE=" + json.dumps(outputs, sort_keys=True))
        self.fail("temporary migration oracle dump")


if __name__ == "__main__":
    unittest.main()
