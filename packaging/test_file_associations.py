"""Static document icon and default-app registration checks; run with Python 3."""

import plistlib
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parent.parent
NS = {"w": "http://wixtoolset.org/schemas/v4/wxs"}


class FileAssociations(unittest.TestCase):
    def test_macos_document_icons_and_roles(self):
        with (ROOT / "packaging/macos/Info.plist.in").open("rb") as source:
            info = plistlib.load(source)
        self.assertEqual(info["CFBundleIconFile"], "wordcraft")
        self.assertTrue(info["CFBundleDocumentTypes"])
        for document in info["CFBundleDocumentTypes"]:
            with self.subTest(document=document["CFBundleTypeName"]):
                self.assertEqual(document["CFBundleTypeIconFile"], info["CFBundleIconFile"])
                self.assertEqual(document["CFBundleTypeRole"], "Editor")
                self.assertEqual(document["LSHandlerRank"], "Alternate")
        package = (ROOT / "packaging/macos/package.sh").read_text()
        self.assertIn('cp "$ROOT/assets/app-icon/wordcraft.icns" "$APP/Contents/Resources/wordcraft.icns"', package)

    def test_windows_candidates_and_icons(self):
        tree = ET.parse(ROOT / "packaging/windows/wordcraft.wxs")
        values = [node.attrib for node in tree.findall(".//w:RegistryValue", NS)]
        capabilities = "Software\\ERPCons\\VanBan\\Capabilities"
        self.assertTrue(any(value.get("Key") == "Software\\RegisteredApplications"
                            and value.get("Name") == "ERPCons.VanBan"
                            and value.get("Value") == capabilities for value in values))
        progids = tree.findall(".//w:ProgId", NS)
        self.assertEqual(len(progids), 10)
        for progid in progids:
            identifier = progid.attrib["Id"]
            extension = "." + identifier.rsplit(".", 1)[1]
            with self.subTest(extension=extension):
                self.assertEqual(progid.attrib["Icon"], "WordcraftExe")
                self.assertEqual(progid.attrib["IconIndex"], "0")
                expected = [
                    ("Software\\Classes\\" + extension + "\\OpenWithProgids", identifier, ""),
                    ("Software\\Classes\\Applications\\wordcraft.exe\\SupportedTypes", extension, ""),
                ]
                # Markdown and plain text are deliberately "Open with" candidates only, not Default apps ones.
                if extension not in (".md", ".txt"):
                    expected.append((capabilities + "\\FileAssociations", extension, identifier))
                for key, name, value in expected:
                    self.assertTrue(any(entry.get("Root") == "HKLM" and entry.get("Key") == key
                                        and entry.get("Name") == name and entry.get("Value") == value for entry in values))

    def test_windows_does_not_set_defaults(self):
        tree = ET.parse(ROOT / "packaging/windows/wordcraft.wxs")
        for node in tree.findall(".//w:RegistryValue", NS):
            key = node.attrib["Key"]
            self.assertNotIn("userchoice", key.lower())
            if key.startswith("Software\\Classes\\."):
                self.assertTrue(key.endswith("\\OpenWithProgids"))
                self.assertTrue(node.attrib.get("Name"))
        self.assertFalse(tree.findall(".//w:Extension", NS))


if __name__ == "__main__":
    unittest.main()
