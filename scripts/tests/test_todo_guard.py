import os
import tempfile
import unittest


class TodoGuardTests(unittest.TestCase):
    def test_untracked_todo_is_reported(self):
        from scripts.todo_guard import scan_repo

        with tempfile.TemporaryDirectory() as td:
            os.makedirs(os.path.join(td, "crates", "x", "src"), exist_ok=True)
            p = os.path.join(td, "crates", "x", "src", "lib.rs")
            with open(p, "w", encoding="utf-8") as f:
                f.write("fn main() {}\n// TODO: do the thing\n")

            res = scan_repo(root=td)
            self.assertEqual(len(res.untracked), 1)
            self.assertIn("TODO", res.untracked[0].text)

    def test_tracked_todo_is_not_reported(self):
        from scripts.todo_guard import scan_repo

        with tempfile.TemporaryDirectory() as td:
            os.makedirs(os.path.join(td, "crates", "x", "src"), exist_ok=True)
            p = os.path.join(td, "crates", "x", "src", "lib.rs")
            with open(p, "w", encoding="utf-8") as f:
                f.write("// TODO(#123): do the thing\n")

            res = scan_repo(root=td)
            self.assertEqual(len(res.untracked), 0)
            self.assertEqual(len(res.all), 1)

    def test_ignored_dirs_are_not_scanned(self):
        from scripts.todo_guard import scan_repo

        with tempfile.TemporaryDirectory() as td:
            os.makedirs(os.path.join(td, "target"), exist_ok=True)
            os.makedirs(os.path.join(td, "crates", "x", "src"), exist_ok=True)
            with open(os.path.join(td, "target", "junk.rs"), "w", encoding="utf-8") as f:
                f.write("// TODO: should be ignored\n")
            with open(
                os.path.join(td, "crates", "x", "src", "lib.rs"), "w", encoding="utf-8"
            ) as f:
                f.write("// TODO(#1): ok\n")

            res = scan_repo(root=td)
            self.assertEqual(len(res.all), 1)
            self.assertEqual(len(res.untracked), 0)


if __name__ == "__main__":
    unittest.main()

