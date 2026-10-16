"""Check template selection without executing the installer or its loop body."""

from pathlib import Path
import re
import subprocess
import unittest


ROOT = Path(__file__).resolve().parent.parent
INSTALLER = ROOT / "scripts/install-neon-sync-launchd.sh"


class NeonSyncLaunchdSelectionTest(unittest.TestCase):
    def test_only_owned_templates_are_selected(self):
        source = INSTALLER.read_text()
        loop = re.search(r"^for template in\b.*?; do$", source, re.MULTILINE | re.DOTALL)
        self.assertIsNotNone(loop, "template selection loop missing")
        # Evaluate only the loop header; never run mkdir, sed, or launchctl.
        selection = 'ROOT="$1"\n' + loop.group() + '\n  printf "%s\\n" "$template"\ndone\n'
        result = subprocess.run(
            ["/bin/bash", "-eu", "-c", selection, "template-selection", str(ROOT)],
            check=True,
            capture_output=True,
            text=True,
        )
        selected = [Path(line) for line in result.stdout.splitlines()]
        expected = {
            "com.memory-platform.neon-sync.plist.template",
            "com.memory-platform.neon-retry.plist.template",
            "com.memory-platform.neon-count-audit.plist.template",
            "com.memory-platform.neon-reconcile.plist.template",
            "com.memory-platform.archive-verify.plist.template",
        }
        self.assertEqual({path.name for path in selected}, expected)
        self.assertEqual(len(selected), len(expected), "duplicate templates selected")
        for path in selected:
            self.assertEqual(path.parent, ROOT / "launchd")
            self.assertTrue(path.is_file(), f"missing template: {path.name}")


if __name__ == "__main__":
    unittest.main()
