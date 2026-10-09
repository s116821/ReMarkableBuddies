"""Current 26-path preservation cases only; packet execution stays refused."""
import unittest
from prepare_packet import partition


class PartitionTests(unittest.TestCase):
    def setUp(self):
        self.pages = ["a7181850-3f03-4bb0-8b9e-01acfc20032e",
                      "5aef884d-d3fc-4635-895a-28237568823d",
                      "d1261cb9-a8c0-4e15-ab24-7a9172b2027b",
                      "f39ae285-3e0c-43dd-b27c-866dff7a24cd",
                      "028b118f-474a-4886-a469-7273b290320c",
                      "fd5afbe2-a79c-461b-8b15-96907d181ab7",
                      "1692961d-ec5c-4f0f-9635-8e6081111dab"]
        self.selection = {"experiment_kind": "open-document-insertion-v1",
                          "document_id": "e7f661f1-db6f-4dfc-854a-b38aff7f75de",
                          "target_page_id": "11111111-1111-4111-8111-111111111111",
                          "before_page_ids": self.pages}
        self.prefix = "/home/root/.local/share/remarkable/xochitl/" + self.selection["document_id"]
        paths = [self.prefix + "." + e for e in ("content", "metadata", "local", "pagedata", "pdf")]
        paths += [self.prefix + "/" + p + ".rm" for p in self.pages[:4]]
        paths += [self.prefix + ".thumbnails/" + p + ".png" for p in self.pages]
        paths += ["/usr/lib/libQt6" + n + ".so.6.10.3" for n in ("Core", "Qml", "Gui", "Quick", "Network")]
        paths += ["/usr/lib/libstdc++.so.6", "/lib/libc.so.6", "/lib/libm.so.6",
                  "/lib/libgcc_s.so.1", "/lib/ld-linux-armhf.so.3"]
        self.files = [{"path": p, "sha256": "a" * 64} for p in paths]

    def test_current_stroked_pair_is_immutable_and_all_paths_retained(self):
        _, _, _, runtime, immutable, mutable = partition(self.selection, self.files)
        self.assertEqual((len(runtime), len(immutable), len(mutable)), (10, 14, 2))
        self.assertIn(self.prefix + "/" + self.pages[1] + ".rm", [e["path"] for e in immutable])
        self.assertIn(self.prefix + ".thumbnails/" + self.pages[1] + ".png", [e["path"] for e in immutable])
        self.assertEqual(len(runtime + immutable + mutable), 26)

    def test_omitting_stroked_pair_or_substituting_paths_refuses(self):
        stroke = self.pages[1]
        old24 = [e for e in self.files if stroke not in e["path"]]
        with self.assertRaises(ValueError):
            partition(self.selection, old24)
        changed = [dict(e) for e in self.files]
        changed[5]["path"] = self.prefix + "/unreviewed.rm"
        with self.assertRaises(ValueError):
            partition(self.selection, changed)

    def test_wrong_order_reused_target_and_legacy_mode_refuse(self):
        reversed_pages = list(reversed(self.pages))
        for selection in ({**self.selection, "before_page_ids": reversed_pages},
                          {**self.selection, "target_page_id": self.pages[1]},
                          {**self.selection, "experiment_kind": "caller-selected-insertion-v1"}):
            with self.subTest(selection=selection), self.assertRaises(ValueError):
                partition(selection, self.files)


if __name__ == "__main__":
    unittest.main()
