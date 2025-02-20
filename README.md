# Hotchkiss-io
Christopher Hotchkiss's personal site / CRM system

## New New URL Scheme

- Top Urls
    Go to / -> look to the first non parent page, redirect there /pages/"page_name"

    /pages/{*page_path}
        -> Each segment walks the content pages, for example
            /resume -> parent-Null + page-resume
            /resume/extras -> parent-resume + page-extras

        If it is special, redirect
            aka /login

            I don't think I need anything special for the rest of it now

        GET -> Gives the content for that page
            GET /pages/{*page_path}/children <-- Gives a view of the child content
            POST /pages/{*page_path}/children <--- Creates a new child page that can be accessed at /pages/{*page_path}
            PATCH /pages/{*page_path}/children <--- Updates the children's order
        PUT -> Updates the content for that page
        DELETE -> Deletes the page
        POST -> Creates a new child page

    //I don't like this, Ideally I'm going to hang this off of the page process above
    GET /attachments/{:page_id} <--- List the page's attachments
    POST /attachments/{:page_id} <--- Upload new attachment

    GET /attachments/"page id"/"attachment name" <--- Get the attachment

        



    /attachments/"page id"/"attachment name"

- / -> Redirect to first tab
- /pages -> Redirect to the first page as per the content_pages
    - /pages/edit -> Edit the content_pages
- /pages/"page_name" -> View a particular page
    - If a particular page is a special page, redirect
- /login -> Login / Register Screen


## User Roles
- Anonymous -> Read Only
- Registered -> Read Only but logged in
- Admin -> Edit rights on the site

## Todo List:
- Figure out how to auto reload the page when the server is restarted
- Allow for editting projects once logged in using markdown / uploads
- Need to figure out how to control navigation tabs without having to pass it around like crazy
- Need to figure out codesigning the server executable
    - Dylan has a great example here: https://github.com/dylanwh/lilguy/blob/main/macos/build.sh
- Fix the sticky footer issue