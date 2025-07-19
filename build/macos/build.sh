#!/bin/bash
# Massive thanks to @dylanwh for the approach
# https://github.com/dylanwh/lilguy/blob/main/macos/build.sh
set -euo pipefail
DOMAIN="io.hotchkiss.web"
EXE="hotchkiss-io"
OUTPUT="target/apple-darwin/release"
VERSION="0.0.1"

rustup target add aarch64-apple-darwin

cargo build --locked --target aarch64-apple-darwin --release

mkdir -p $OUTPUT
cp target/aarch64-apple-darwin/release/$EXE $OUTPUT/$EXE

xcrun codesign \
    --sign "G53N9PU948" \
    --timestamp \
    --options runtime \
    --entitlements build/macos/entitlements.plist \
    $OUTPUT/$EXE

pkgbuild --root $OUTPUT \
    --identifier "$DOMAIN" \
    --version "$VERSION" \
    --install-location /Applications \
    --sign "G53N9PU948" \
    target/$EXE.pkg

productbuild \
    --distribution build/macos/Resources/Distribution.xml \
    --resources build/macos/Resources/ --package-path target/ unsigned-$EXE.pkg

productsign --sign "G53N9PU948" unsigned-$EXE.pkg $EXE.pkg

xcrun notarytool submit $EXE.pkg \
    --keychain-profile "AppPwdNotarizID" \
    --wait

xcrun stapler staple $EXE.pkg

mv $EXE.pkg "$EXE-$VERSION.pkg"
