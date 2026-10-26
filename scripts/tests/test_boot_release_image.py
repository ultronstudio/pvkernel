"""Exercise success and failure of production-boot observation with real children."""

import importlib.util
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("boot_release_image", REPO / "scripts/boot_release_image.py")
boot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(boot)


class ProductionBootTests(unittest.TestCase):
    def run_child(self, log, *, after="", exit_early=False, timeout=1.5):
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / "serial.log"
            body = (
                "import pathlib,sys,time\n"
                "p=pathlib.Path(sys.argv[1])\n"
                f"p.write_text({log!r})\n"
            )
            if after:
                body += f"time.sleep(0.05)\nwith p.open('a') as f: f.write({after!r})\n"
            if not exit_early:
                body += "time.sleep(30)\n"
            with contextlib.redirect_stdout(io.StringIO()):
                boot.observe([sys.executable, "-c", body, str(serial)], serial, timeout=timeout, settle=0.2)

    def test_normal_pid1_boot_remains_running(self):
        self.run_child("\n".join(boot.MARKERS))

    def test_timeout_without_cpl3_marker_fails(self):
        with self.assertRaisesRegex(RuntimeError, "timed out"):
            self.run_child("\n".join(boot.MARKERS[:-2]), timeout=0.35)

    def test_early_exit_is_not_a_success_even_with_markers(self):
        with self.assertRaisesRegex(RuntimeError, "exited"):
            self.run_child("\n".join(boot.MARKERS), exit_early=True)

    def test_panic_after_marker_fails_during_observation(self):
        with self.assertRaisesRegex(RuntimeError, "fatal"):
            self.run_child("\n".join(boot.MARKERS), after="\npvKernel PANIC\n")

    def test_user_fault_after_marker_fails(self):
        with self.assertRaisesRegex(RuntimeError, "fatal"):
            self.run_child("\n".join(boot.MARKERS), after="\n[exception] page fault\n")

    def test_test_enabled_image_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "test-only"):
            self.run_child("\n".join(boot.MARKERS) + "\n[initrd-test] PID 1 CPL3 marker accepted")

    def test_wrong_pid_or_out_of_order_markers_do_not_pass(self):
        self.assertFalse(boot.reached_init("\n".join(boot.MARKERS).replace("PID 1", "PID 2")))
        self.assertFalse(boot.reached_init("\n".join(reversed(boot.MARKERS))))
