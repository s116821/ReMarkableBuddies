# Historical handwriting fixture

This is genuine handwritten correspondence under printed letterhead, not a
synthetic tablet-stroke fixture or a technical notebook benchmark.

Source: [National Park Service asset c922fc45-34de-492d-802e-2080de9b466d](https://npgallery.nps.gov/AssetDetail/c922fc45-34de-492d-802e-2080de9b466d),
M. J. Sherman to Alice Mary Longfellow, 24 January 1900. The primary asset's
Constraints Information states **Public domain**. Credit: Courtesy of National
Park Service, Longfellow House-Washington's Headquarters National Historic Site;
Archives Number 1007.001/002.003-001#085.

The original download is [original.jpg](https://npgallery.nps.gov/GetAsset/c922fc45-34de-492d-802e-2080de9b466d/original.jpg).
The tracked `original.jpg` is571,563 bytes, SHA256
`6c309acb4749bc3123e0a8cba1544f6229b5b24975e40c48ddab80a05572b53c`.
The tracked `page.png` is3,147,147 bytes,1822x2324 RGB, SHA256
`a4a5b635299491db594d2a69f9dacb731060f757f1c69573fe3e320fe2085f32`.
Only JPEG decode, RGB conversion and lossless PNG encoding were performed; no
crop, resize, retouch or generated handwriting. The adapter accepts PNG only.

`prepare.py` verifies both fixed files and exact decoded RGB equality. With
Python and Pillow installed, run `python tests/fixtures/notebook-handwriting/prepare.py`.
It does not download, overwrite or call a model. Re-encoding with another Pillow
version may produce different PNG bytes with identical pixels; identity remains
bound to the actual fixed bytes, not a presumed encoder output.

`expected-facts.json` records facts visually selected before model evaluation.
It is a semantic reference, not a complete exact transcription. The primary
catalog transcription omits printed header details and omits the visible word
“you” in the closing thanks sentence. Inspect the scan when judging wording.
Neither reference is sent to the model by the maintained caller. Retain actual
recognition errors and uncertainty; one observation cannot establish general
accuracy, nonmemorization, source freshness or native/runtime qualification.
