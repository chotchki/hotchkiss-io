# Hotchkiss-io
Christopher Hotchkiss's personal site / CRM system

## New URL Scheme
- / -> Redirect to /pages
- /pages -> Redirect to the first page as per the content_pages
- /pages/"page_name" -> View a particular page
    - If a particular page is a special page, redirect
- /login -> Login / Register Screen


## User Roles
- Anonymous -> Read Only
- Registered -> Read Only but logged in
- Admin -> Edit rights on the site

## Todo List:
1. Figure out how to auto reload the page when the server is restarted
2. Allow for editting projects once logged in using markdown / uploads
3. Need to figure out site content and consult with my consultant
4. Need to figure out how to control navigation tabs without having to pass it around like crazy
5. Need to figure out codesigning the server executable
    1. Dylan has a great example here: https://github.com/dylanwh/lilguy/blob/main/macos/build.sh