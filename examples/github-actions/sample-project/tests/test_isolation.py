"""What untrusted pull-request code should not be able to do from the sandbox.

Run with the workflow's deny_all egress policy these pass. On a sandbox with
default egress, test_no_outbound_connection fails if the host has a route out,
which is the point: it proves the policy, not the code.
"""

import socket
import unittest
from pathlib import Path


class Isolation(unittest.TestCase):
    def test_no_outbound_connection(self):
        with self.assertRaises(OSError):
            socket.create_connection(("1.1.1.1", 53), timeout=3).close()

    def test_runner_credentials_are_absent(self):
        # The action never uploads .git (a checkout's credential lives there).
        self.assertFalse(Path("/workspace/.git").exists())


if __name__ == "__main__":
    unittest.main()
