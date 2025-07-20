# Hotchkiss-io
Christopher Hotchkiss's personal site / CRM system

[![Test and Coverage](https://github.com/chotchki/hotchkiss-io/actions/workflows/test_and_coverage.yml/badge.svg?branch=main)](https://github.com/chotchki/hotchkiss-io/actions/workflows/test_and_coverage.yml) [![codecov](https://codecov.io/github/chotchki/hotchkiss-io/branch/main/graph/badge.svg?token=APIMLQTEDX)](https://codecov.io/github/chotchki/hotchkiss-io)

## User Roles
- Anonymous -> Read Only
- Registered -> Read Only but logged in
- Admin -> Edit rights on the site

## Todo List:
- [ ] Update building, I have a local git runner on my server, might as well use it
- [ ] I really need some code coverage, I left this project half done and I'm not sure if I've broken anything
- [ ] Need to figure out codesigning the server executable
- - Dylan has a great example here: https://github.com/dylanwh/lilguy/blob/main/macos/build.sh
- [ ] Consider migrating to daisyUI so that the UI parts work okay
- [ ] Need to figure out attachment resizing plus caching so I can really start uploading images / stl files.
- [ ] Probably will need to move behind Cloudflare's AI bot protection but that will change my server start up process
- [ ] Fix the sticky footer issue
