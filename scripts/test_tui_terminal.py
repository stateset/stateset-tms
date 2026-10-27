"""Screen-state assertions must survive terminal diffs and transport chunking."""
import unittest
from tui_terminal import Screen


class ScreenTests(unittest.TestCase):
    def test_diff_replaces_stale_readiness_and_review(self):
        screen = Screen(3, 40)
        screen.feed(b'1 shown\x1b[2;1Hy confirms and saves')
        self.assertIn('1 shown', screen.text)
        screen.feed(b'\x1b[H\x1b[2KLoading\x1b[2;1H\x1b[2KEnter reviews')
        self.assertNotIn('shown', screen.text)
        self.assertNotIn('y confirms', screen.text)
        screen.feed(b'\x1b[H1 shown\x1b[K')
        self.assertIn('1 shown', screen.text)
        self.assertNotIn('Loading', screen.text)

    def test_every_byte_boundary_preserves_utf8_and_control_sequences(self):
        payload = 'wrong\x1b[2J\x1b[HClaim work\x1b[2;1H\x1b[1m東京é\x1b[0m\x1b]0;private title\x1b\\'.encode()
        complete = Screen(3, 25); complete.feed(payload)
        for cut in range(len(payload) + 1):
            split = Screen(3, 25); split.feed(payload[:cut]); split.feed(payload[cut:])
            self.assertEqual(split.text, complete.text, cut)
        self.assertNotIn('private title', complete.text)
        self.assertTrue(complete.text.startswith('Claim work\n東京é'))

    def test_cursor_movement_erasure_and_saved_position(self):
        screen = Screen(3, 12)
        screen.feed(b'abc\x1b[s\x1b[3;8Hother\x1b[uZ\x1b[1D!\x1b[2;1H123\rX\nY')
        self.assertEqual(screen.text.splitlines()[0], 'abc!')
        self.assertEqual(screen.text.splitlines()[1], 'X23')
        screen.feed(b'\x1b[2J')
        self.assertFalse(screen.text.strip())


if __name__ == '__main__':
    unittest.main()
