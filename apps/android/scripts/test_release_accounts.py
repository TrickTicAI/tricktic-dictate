"""Offline Play artifact regressions; no SDK, keys, signing or store calls."""
import io
import unittest
import zipfile

import release


class PlayAccountGateTests(unittest.TestCase):
    def inspect(self, files):
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, 'w') as bundle:
            for name, content in files.items():
                bundle.writestr(name, content)
        with zipfile.ZipFile(archive) as bundle:
            release.inspect_play_dex(bundle)

    def test_account_free_dictation_passes(self):
        self.inspect({'base/dex/classes.dex': b'dex\nLcom/metrix/writer/ui/Account;\x00Lcom/metrix/writer/sync/AccountSync;\x00',
                      'base/dex/classes2.dex': b'Lcom/metrix/writer/PhoneEngine;\x00'})

    def test_authentication_implementations_are_rejected(self):
        for descriptor in [b'Lcom/google/firebase/auth/FirebaseAuth;',
                           b'Landroidx/credentials/CredentialManager;',
                           b'Lcom/google/android/libraries/identity/googleid/GetSignInWithGoogleOption;',
                           b'Lcom/metrix/writer/account/FirebaseGoogleIdentity;']:
            with self.subTest(descriptor=descriptor):
                with self.assertRaisesRegex(ValueError, 'Unapproved sign-in'):
                    self.inspect({'base/dex/classes2.dex': descriptor})

    def test_sign_in_in_another_module_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'account/dex/classes.dex'):
            self.inspect({'base/dex/classes.dex': b'Lcom/metrix/writer/PhoneEngine;',
                          'account/dex/classes.dex': b'Lcom/google/firebase/auth/FirebaseAuth;'})

    def test_existing_updater_exclusion_still_applies(self):
        with self.assertRaisesRegex(ValueError, 'self-update'):
            self.inspect({'base/dex/classes.dex': b'Lcom/metrix/writer/AndroidUpdater;'})


if __name__ == '__main__':
    unittest.main()
