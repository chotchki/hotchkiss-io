#!/bin/bash
# Massive thanks to @dylanwh for the approach
# https://github.com/dylanwh/lilguy/blob/main/macos/build.sh
set -euo pipefail
DOMAIN="io.hotchkiss.web"
EXE="hotchkiss-io"
OUTPUT="target/apple-darwin/release"

rustup target add aarch64-apple-darwin

cargo build --locked --target aarch64-apple-darwin --release

mkdir -p $OUTPUT/Hotchkiss-IO.app
mkdir -p $OUTPUT/Hotchkiss-IO.app/Contents/MacOS
mkdir -p $OUTPUT/Hotchkiss-IO.app/Contents/Resources

cp target/aarch64-apple-darwin/release/$EXE $OUTPUT/Hotchkiss-IO.app/Contents/MacOS/$EXE
sed -e "s;%VERSION%;$VERSION;g" build/macos/Info.plist > $OUTPUT/Hotchkiss-IO.app/Contents/Info.plist
cp build/macos/HotchkissLogox1024.icns $OUTPUT/Hotchkiss-IO.app/Contents/Resources

xcrun codesign \
    --sign "G53N9PU948" \
    --timestamp \
    --options runtime \
    --entitlements build/macos/entitlements.plist \
    $OUTPUT/Hotchkiss-IO.app/Contents/MacOS/$EXE

pkgbuild --root $OUTPUT \
    --identifier "$DOMAIN" \
    --component-plist build/macos/pkgbuild.plist \
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
