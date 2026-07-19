#!/usr/bin/env bash
# Generate a few DISTINCT tiny EPUBs for the bulk-manga-ingest tests (Phase DW.6).
# Each volume carries a distinct dc:identifier/title so its bytes hash differently —
# otherwise the content-hash dedup would (correctly) skip vols 2+ as the same file.
# Filenames encode the volume number so parse_volume orders them. Output:
# tests/fixtures/manga/series-v0N.epub.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
outdir="$repo/tests/fixtures/manga"
rm -rf "$outdir" && mkdir -p "$outdir"

# A tiny valid PNG cover (2x2), shared — the cover-extraction path just needs SOME image.
png='iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEUlEQVR4nGP8z8Dwn4EIwDiKAgB5ZwQBIsN9UwAAAABJRU5ErkJggg=='

for n in 1 2 3; do
  src="$here/vol$n"
  rm -rf "$src" && mkdir -p "$src/META-INF"
  printf 'application/epub+zip' > "$src/mimetype"

  cat > "$src/META-INF/container.xml" <<'XML'
<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>
XML

  cat > "$src/content.opf" <<XML
<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id" xml:lang="en">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="id">urn:uuid:hio-test-manga-vol$n</dc:identifier>
    <dc:title>Test Series Volume $n</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="cover" href="cover.png" media-type="image/png" properties="cover-image"/>
    <item id="p1" href="page1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine page-progression-direction="rtl"><itemref idref="p1"/></spine>
</package>
XML

  cat > "$src/nav.xhtml" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head>
<body><nav epub:type="toc"><ol><li><a href="page1.xhtml">Page 1</a></li></ol></nav></body>
</html>
XML

  cat > "$src/page1.xhtml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Volume $n</title></head>
<body><h1>Test Series Volume $n</h1><p>bulk ingest fixture, volume $n.</p></body></html>
XML

  printf '%s' "$png" | base64 -d > "$src/cover.png"

  out="$outdir/series-v0$n.epub"
  rm -f "$out"
  ( cd "$src" && zip -X0 "$out" mimetype >/dev/null \
      && zip -Xr9 "$out" META-INF content.opf nav.xhtml cover.png page1.xhtml >/dev/null )
  rm -rf "$src"
  echo "wrote $out"
done
