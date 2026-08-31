import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from runner import load_calibration, mjpeg_frames


class CuVslamMonoContractTests(unittest.TestCase):
    def test_calibration_scales_without_changing_fisheye_coefficients(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "calibration.json"
            path.write_text(
                json.dumps(
                    {
                        "calibration_id": "camera-1",
                        "image": {"width_px": 1920, "height_px": 1080},
                        "fisheye": {
                            "camera_matrix": [[1000, 0, 960], [0, 1002, 540], [0, 0, 1]],
                            "distortion": [-0.1, 0.02, -0.03, 0.04],
                        },
                    }
                ),
                encoding="utf-8",
            )
            calibration = load_calibration(path).scaled(1280, 720)
            self.assertEqual(calibration.focal, (1000 * 2 / 3, 1002 * 2 / 3))
            self.assertEqual(calibration.principal, (640, 360))
            self.assertEqual(calibration.distortion, (-0.1, 0.02, -0.03, 0.04))

    def test_mjpeg_contract_preserves_leash_sequence_and_timestamps(self):
        jpeg = b"\xff\xd8fixture\xff\xd9"
        stream = io.BytesIO(
            b"--leashframe\r\n"
            b"Content-Type: image/jpeg\r\n"
            + f"Content-Length: {len(jpeg)}\r\n".encode()
            + b"X-Leash-Sequence: 42\r\n"
            b"X-Leash-Captured-At-Ms: 1234\r\n"
            b"X-Leash-Monotonic-Ns: 9000\r\n\r\n"
            + jpeg
            + b"\r\n"
        )
        frame = next(mjpeg_frames(stream))
        self.assertEqual(frame.jpeg, jpeg)
        self.assertEqual(frame.sequence, 42)
        self.assertEqual(frame.captured_at_ms, 1234)
        self.assertEqual(frame.monotonic_ns, 9000)


if __name__ == "__main__":
    unittest.main()
