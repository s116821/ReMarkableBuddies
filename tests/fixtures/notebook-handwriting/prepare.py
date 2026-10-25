"""Verify fixed attributed fixture bytes and unchanged decoded RGB pixels."""
from pathlib import Path
import hashlib
from PIL import Image

root = Path(__file__).resolve().parent
for name, expected in [('original.jpg','6c309acb4749bc3123e0a8cba1544f6229b5b24975e40c48ddab80a05572b53c'),('page.png','a4a5b635299491db594d2a69f9dacb731060f757f1c69573fe3e320fe2085f32')]:
    assert hashlib.sha256((root/name).read_bytes()).hexdigest()==expected
with Image.open(root/'original.jpg') as original, Image.open(root/'page.png') as png:
    assert original.size==png.size==(1822,2324)
    assert original.convert('RGB').tobytes()==png.convert('RGB').tobytes()
print('Fixed historical fixture bytes and decoded RGB equality verified')
