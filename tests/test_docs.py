"""Structural guards for the required plan and evidence ledger."""

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parent.parent


class DocumentationTests(unittest.TestCase):
    def test_all_phases_and_task_acceptance_criteria_are_present(self):
        plan = (ROOT / "TODO.MD").read_text(encoding="utf-8")
        phases = re.findall(r"^## Phase (\d+)\s+—", plan, re.MULTILINE)
        self.assertEqual(phases, [str(number) for number in range(13)])
        tasks = [line for line in plan.splitlines() if re.match(r"^- \[[ x]\] ", line)]
        self.assertGreater(len(tasks), 50)
        for task in tasks:
            with self.subTest(task=task):
                self.assertRegex(task, r"; accept: .+; tests?: .+")

    def test_every_completed_task_has_passing_evidence(self):
        plan = (ROOT / "TODO.MD").read_text(encoding="utf-8")
        report = (ROOT / "docs" / "TEST_REPORT.md").read_text(encoding="utf-8")
        rows = report.splitlines()
        for task in (line for line in plan.splitlines() if line.startswith("- [x] ")):
            evidence = re.search(r"; tests?: (.+)", task)
            self.assertIsNotNone(evidence, task)
            ids = re.findall(r"\b[A-Z][A-Z0-9]*(?:-[A-Z0-9]+)+\b", evidence.group(1))
            self.assertTrue(ids, task)
            for identifier in ids:
                with self.subTest(identifier=identifier):
                    matches = [row for row in rows if row.startswith(f"| {identifier} |")]
                    self.assertEqual(len(matches), 1, f"Missing evidence row for {identifier}")
                    self.assertRegex(matches[0], r"\| PASS(?: \([^|]*\))? \|$")

    def test_architecture_covers_essential_boundaries(self):
        architecture = (ROOT / "docs" / "ARCHITECTURE.md").read_text(encoding="utf-8").lower()
        for word in ("risc-v", "wasm", "storage", "security", "network", "webgpu"):
            with self.subTest(word=word):
                self.assertIn(word, architecture)
        self.assertTrue((ROOT / "README.md").is_file())
        self.assertTrue((ROOT / "THIRD_PARTY_NOTICES.md").is_file())


if __name__ == "__main__":
    unittest.main()
