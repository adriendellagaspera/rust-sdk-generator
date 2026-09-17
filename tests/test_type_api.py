import unittest

from rust_sdk_generator import Type, parse_type


class PublicTypeApiTests(unittest.TestCase):
    def test_parse_type_is_structural_and_public(self):
        syntax = parse_type("Option<Vec<String>>")
        self.assertIsInstance(syntax, Type)
        inner = syntax.unary("Option")
        self.assertIsNotNone(inner)
        self.assertEqual(inner.constructor, "Vec")
        self.assertEqual(inner.arguments[0].spelling, "String")

    def test_parse_type_rejects_statement_suffixes(self):
        with self.assertRaises(ValueError):
            parse_type("Option<String>; fn injected() {}")


if __name__ == "__main__":
    unittest.main()
