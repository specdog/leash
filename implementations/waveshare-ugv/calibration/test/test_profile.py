from __future__ import annotations

import copy
import unittest

from profile import canonical_bytes, validate
from support import candidate_profile


class ProfileTests(unittest.TestCase):
    def desired_candidate(self) -> dict:
        profile = candidate_profile()
        profile["measurement"] = {
            "procedure_revision": "issue-166-v1",
            "measured_at": "2026-07-10T00:00:00Z",
            "acceptance_manifest_sha256": None,
        }
        return profile

    def test_candidate_allows_null_acceptance_manifest_digest(self):
        profile = self.desired_candidate()

        self.assertEqual(validate(profile, require_values=True), profile)

    def test_accepted_profile_requires_lowercase_manifest_digest(self):
        profile = self.desired_candidate()
        profile["status"] = "accepted"

        with self.assertRaisesRegex(ValueError, "acceptance_manifest_sha256"):
            validate(profile, require_accepted=True)

        profile["measurement"]["acceptance_manifest_sha256"] = "A" * 64
        with self.assertRaisesRegex(ValueError, "acceptance_manifest_sha256"):
            validate(profile, require_accepted=True)

        profile["measurement"]["acceptance_manifest_sha256"] = "a" * 64
        self.assertEqual(validate(profile, require_accepted=True), profile)

    def test_legacy_evidence_digest_list_is_rejected(self):
        profile = self.desired_candidate()
        profile["measurement"]["evidence_sha256"] = []

        with self.assertRaisesRegex(ValueError, "evidence_sha256"):
            validate(profile, require_values=True)

    def test_manifest_digest_does_not_change_canonical_calibration_bytes(self):
        candidate = self.desired_candidate()
        accepted = copy.deepcopy(candidate)
        accepted["status"] = "accepted"
        accepted["measurement"]["acceptance_manifest_sha256"] = "b" * 64

        self.assertEqual(canonical_bytes(candidate), canonical_bytes(accepted))


if __name__ == "__main__":
    unittest.main()
