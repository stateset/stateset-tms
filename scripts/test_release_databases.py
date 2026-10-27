"""Release checks must not share destructive test or interface databases."""
import json
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import release_databases as gate


class ReleaseDatabaseTests(unittest.TestCase):
    def setUp(self):
        self.env = {name: f"postgres://fixture:private@localhost/{n}"
                    for n, name in enumerate((*gate.DATABASES, gate.MCP_DATABASE))}

    def inspector(self, name, url):
        return {"version": 160015, "cluster": "123456", "database": name, "populated": False}

    def test_every_release_interface_is_required_before_any_connection(self):
        for name in self.env:
            with self.subTest(name=name):
                inspect = Mock()
                env = {**self.env, name: " "}
                with self.assertRaisesRegex(ValueError, name):
                    gate.check(env, mcp=True, inspector=inspect)
                inspect.assert_not_called()

    def test_server_identity_rejects_aliases_for_every_pair(self):
        names = list(self.env)
        for i, first in enumerate(names):
            for second in names[i + 1:]:
                with self.subTest(first=first, second=second):
                    def inspect(name, url):
                        value = self.inspector(name, url)
                        if name in (first, second):
                            value["database"] = "same_db"
                        return value
                    with self.assertRaisesRegex(ValueError, "same database"):
                        gate.check(self.env, mcp=True, inspector=inspect)

    def test_same_database_name_on_different_clusters_is_distinct(self):
        def inspect(name, url):
            return {**self.inspector(name, url), "cluster": name, "database": "fixture"}
        self.assertEqual(gate.check(self.env, mcp=True, inspector=inspect), 4)

    def test_freshness_does_not_confuse_an_existing_unit_test_database(self):
        def inspect(name, url):
            return {**self.inspector(name, url), "populated": name == "TEST_DATABASE_URL"}
        self.assertEqual(gate.check(self.env, mcp=True, fresh=True, inspector=inspect), 4)
        for occupied in (*gate.DATABASES[1:], gate.MCP_DATABASE):
            def inspect(name, url):
                return {**self.inspector(name, url), "populated": name == occupied}
            with self.assertRaisesRegex(ValueError, "fresh database"):
                gate.check(self.env, mcp=True, fresh=True, inspector=inspect)

    def test_identity_and_postgres_version_are_validated(self):
        good = self.inspector("fixture", "")
        for value in [None, {}, {**good, "version": True}, {**good, "version": 150000},
                      {**good, "version": 170000}, {**good, "cluster": ""},
                      {**good, "database": ""}, {**good, "populated": "false"}]:
            with self.subTest(value=value), patch("release_databases.subprocess.run",
                    return_value=SimpleNamespace(stdout=json.dumps(value))):
                with self.assertRaises(ValueError):
                    gate.inspect("TEST_DATABASE_URL", "postgres://fixture:private@localhost/fixture")

    def test_credentials_never_enter_argv_or_errors(self):
        secret = "postgres://fixture:private@localhost/fixture"
        with patch("release_databases.subprocess.run",
                   return_value=SimpleNamespace(stdout=json.dumps(self.inspector("fixture", "")))) as run:
            gate.inspect("TEST_DATABASE_URL", secret)
            args, kwargs = run.call_args
            self.assertNotIn(secret, args[0])
            self.assertNotIn("private", " ".join(args[0]))
            self.assertEqual(kwargs["env"]["PGPASSWORD"], "private")
            self.assertEqual(kwargs["timeout"], 15)
        for error in (subprocess.CalledProcessError(1, secret, stderr=secret),
                      subprocess.TimeoutExpired(secret, 15), OSError(secret)):
            with patch("release_databases.subprocess.run", side_effect=error):
                with self.assertRaises(ValueError) as raised:
                    gate.inspect("TEST_DATABASE_URL", secret)
                self.assertNotIn(secret, str(raised.exception))

    def test_url_password_variants_keep_tls_and_socket_options(self):
        url = "postgresql://user:p%40ss@[::1]:5432/db?sslmode=verify-full&host=%2Ftmp&password=override%21"
        with patch("release_databases.subprocess.run", return_value=SimpleNamespace(
                stdout=json.dumps(self.inspector("fixture", "")))) as run:
            gate.inspect("TEST_DATABASE_URL", url)
            args, kwargs = run.call_args
            self.assertEqual(args[0][2], "postgresql://user@[::1]:5432/db?sslmode=verify-full&host=%2Ftmp")
            self.assertEqual(kwargs["env"]["PGPASSWORD"], "override!")


if __name__ == "__main__":
    unittest.main()
