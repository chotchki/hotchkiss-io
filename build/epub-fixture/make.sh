#!/usr/bin/env bash
# Regenerate the minimal EPUB fixture for the reader tests (Phase DV).
# A valid EPUB: the `mimetype` entry MUST be first AND stored (uncompressed);
# everything else is a normal deflated zip entry. RTL spine so the manga path is
# exercised. Output: tests/fixtures/test.epub.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
src="$here/src"

rm -rf "$src" && mkdir -p "$src/META-INF"
printf 'application/epub+zip' > "$src/mimetype"

cat > "$src/META-INF/container.xml" <<'XML'
<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>
XML

cat > "$src/content.opf" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id" xml:lang="en">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="id">urn:uuid:hio-test-epub-0001</dc:identifier>
    <dc:title>Test Manga Volume</dc:title>
    <dc:language>en</dc:language>
    <meta property="rendition:layout">pre-paginated</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="cover" href="cover.png" media-type="image/png" properties="cover-image"/>
    <item id="p1" href="page1.xhtml" media-type="application/xhtml+xml"/>
    <item id="p2" href="page2.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine page-progression-direction="rtl">
    <itemref idref="p1"/>
    <itemref idref="p2"/>
  </spine>
</package>
XML

cat > "$src/nav.xhtml" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head>
<body><nav epub:type="toc"><ol><li><a href="page1.xhtml">Page 1</a></li></ol></nav></body>
</html>
XML

# A tiny valid PNG cover (2x2) — enough for the DV.10 cover-extraction test.
printf '%s' 'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEUlEQVR4nGP8z8Dwn4EIwDiKAgB5ZwQBIsN9UwAAAABJRU5ErkJggg==' | base64 -d > "$src/cover.png"

for n in 1 2; do
  cat > "$src/page$n.xhtml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Page $n</title></head>
<body><h1>HELLO-EPUB Page $n</h1><p>foliate test content page $n.</p></body></html>
XML
done

out="$repo/tests/fixtures/test.epub"
rm -f "$out"
( cd "$src" && zip -X0 "$out" mimetype >/dev/null \
    && zip -Xr9 "$out" META-INF content.opf nav.xhtml cover.png page1.xhtml page2.xhtml >/dev/null )
echo "wrote $out"
