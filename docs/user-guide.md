# IronCall user guide

IronCall is a desktop client for HTTP APIs. Requests are YAML files in a folder you can
commit to git, and nothing leaves your machine except the requests you send.

New here? On the welcome screen, choose **Try the sample workspace…** and pick an empty
folder. It calls IronCall's built-in echo server, so every example works offline.

## The window

- **Toolbar:** the workspace menu, the environment picker, `{{ }}` (variables quick
  look), **Run**, **Recent**, **History**, the theme toggle and **Settings**.
- **Sidebar:** the workspace tree, a filter box, and scratch requests and folders at the
  bottom.
- **Editor:** tabs of open requests. A single click in the tree opens a preview tab (in
  italics) that the next click replaces; double-click or edit it to keep it open.
- **Response:** to the right of the request.

## Workspaces

A workspace is a folder with an `ironcall.yaml`. From the workspace menu: **Open
workspace…**, **Create workspace…** (choose an empty folder, or one with your code), or
pick a recent one.

- **Create** requests and folders from the tree's context menu, or with the buttons
  next to the filter (`+` new request, `▣` new folder, `⇩` import). **Drag** to reorder
  or move them.
- **Rename** with F2 or the context menu. Only the display name changes; the file keeps
  its name, so git history stays readable. *Rename file to match name* (*Rename folder
  to match name* for folders) renames the file too. Open tabs follow.
- **Duplicate** copies a request or folder (with a new ID) right after the original.
  **Open in external editor** opens a request's YAML in your editor; **Reveal in file
  manager** shows a file or folder on disk.
- **Delete** moves workspace items to the system trash; an open tab shows "deleted on
  disk".
- **Edits made outside IronCall** (an editor, `git pull`) show up straight away. If you
  have unsaved changes to the same request, a bar offers **Keep mine**, **Take theirs**
  or **Compare**.
- A file IronCall can't read (bad YAML, merge-conflict markers) is shown with a ⚠ and
  the error. It's never overwritten; fix it in your editor. A file written by a newer
  IronCall opens read-only.
- **Folder and collection settings** are in the tree's context menu: **Folder
  variables…** / **Collection variables…**, **Auth & default headers…** (inherited by
  every request inside) and **Edit description…**. The workspace menu has the
  collection's too (**Collection variables…**, **Collection auth & headers…**), plus
  **Global variables…** and **Close workspace**. Right-clicking the empty space below
  the tree opens the collection's menu.

See the [file format reference](file-format.md) for what's in each file.

### Scratch requests

Requests outside any workspace, for quick experiments. They're kept between sessions.
**Ctrl+N** opens one; **Save to workspace…** moves it into a folder.

Scratch requests can be grouped in **scratch folders**. Right-click the Scratch heading,
a scratch folder or a scratch request for **New request** and **New folder**; drag a
request or folder into a folder, or choose **Move to top level**; F2 or **Rename** renames
a folder. Scratch folders live on this machine only (they're not files in a workspace),
so they can't be shared. **Deleting scratch items is permanent**: unlike workspace
items, they don't go to the system trash. Deleting a folder also deletes the requests
in it, so IronCall asks first; deleting a single scratch request doesn't.

## Building a request

- **Method and URL** at the top. Type query parameters in the URL or in **Params**;
  both stay in sync. `:name` or `{name}` in the path adds a path parameter.
- **Headers**: headers added automatically (content type, auth, defaults from folders)
  are listed below yours, and yours win.
- **Auth**: *Inherit* (the default) uses the nearest folder's or the collection's auth.
  Basic, Bearer and API key (header or query) are supported.
- **Body**: JSON, text, XML, form (URL-encoded), multipart (text and file parts), or a
  binary file. Files are read when you send.
- **Vars**: variables for this request only.
- **Tests**: assertions and extractions (see [Testing responses](#testing-responses)).
- **Docs**: a Markdown description.
- **Settings**: timeout, redirects, HTTP version, TLS verification, a fresh connection,
  raw (unencoded) URL, proxy bypass. Unset values use Settings › Requests.

**Preview** shows exactly what will be sent, with variables resolved and secrets masked.
**Send** with **Ctrl+Enter**; **Esc** cancels. **Ctrl+S** saves.

### Importing and exporting cURL

**Paste a cURL command into the URL bar** to turn it into a request (bash, or Chrome's
*Copy as cURL (cmd)*). Flags IronCall doesn't support are listed, never dropped
silently. **Import cURL / OpenAPI…** in the workspace menu also imports OpenAPI 3.x and
Swagger 2.0 specs, with a preview first.

**Copy as cURL…** (tab or tree context menu) exports bash or PowerShell, with
`{{variables}}` kept, resolved with secrets masked (the default), or fully resolved.

## Variables and environments

Write `{{name}}` anywhere in a request. Defined variables are highlighted; undefined
ones are red. Hover to see the value and where it comes from. The `{{ }}` button lists
every variable the current request can see.

Variables come from, lowest priority first: global, collection, the active environment,
folders, the request, and runtime values from extractions. See
[Variables](file-format.md#variables) for built-in helpers such as `{{$uuid}}`.

**Environments** (Dev, Prod…) are picked in the toolbar and edited in **Environments…**.

### Secrets

Mark a variable **secret** and its value is stored on this machine only, never in the
YAML. Choose where per workspace in **Secret storage…**: the OS keychain (default), an
encrypted file (passphrase once per session), or a plaintext file in `.ironcall/`. The
choice is kept on this machine, not in the workspace, so a shared folder can't make it
for you. When a teammate clones the workspace, a banner lists the secrets they still need
to enter.

The keychain needs a running Secret Service provider (GNOME Keyring or KWallet). Without
one, IronCall says so and offers the encrypted file. Workspaces that chose a backend with
IronCall 0.9.0 (which kept the choice in `.ironcall/local.yaml`) use the keychain after
upgrading until you pick again in **Secret storage…**; a banner reminds you.

Secret values, and headers such as `Authorization` and `Cookie`, are masked in history,
logs, the timeline and masked cURL exports. Add your own sensitive headers in
Settings › Privacy.

## Responses

The status, time and size are at the top. The tabs:

- **Body**: pretty or raw, with search (Ctrl+F). Large bodies (hundreds of MB) open
  quickly and scroll smoothly. **Save response…** writes the body to a file.
- **Preview**: images, PDFs and HTML (sandboxed: an HTML preview can't load anything).
- **Headers**, **Timing** (DNS, connect, TLS, waiting, download; *reused* when the
  connection was reused), **TLS** (certificate and protocol), **Timeline** (a
  `curl -v`-style log), **Request** (what was actually sent).
- **Tests**: assertion results and extracted values, when the request has any.

The tabs appear in this order: Body, Preview (when there's something to preview),
Headers, Tests, Timing, TLS (for HTTPS), Timeline, Request. Binary bodies are shown as
hex. A body too large to pretty-print quickly offers **Pretty-print anyway**.
**Recent** in the toolbar lists the active tab's last sends.

### History

**Ctrl+H** opens the history of sends, searchable, for this workspace or all. Open an
entry in a new tab, resend it, or look at the stored response. Retention is set in
**History settings…**; response bodies can be left out per workspace.

### Cookies

Responses' cookies are kept per workspace and sent automatically. **Cookies…** in the
workspace menu shows and edits them.

## Testing responses

In a request's **Tests** tab:

- **Assertions** check the response every time you send it: the status, a header, a
  JSONPath into the body (*JSON body*), the body text, the time or the size, with checks
  such as *equals*, *contains*, *matches* (a regex), *type is* and *is one of* (type
  `200, 201`). Expected values can use `{{variables}}`.
- **Extract into variables** saves part of a response (a JSONPath, a header, a cookie or a regex
  match) into a **runtime variable**. Runtime variables override everything else until
  the app closes, so a login request can pass its token to the requests after it. Save
  one permanently from the `{{ }}` quick look (**Save** puts it in the active
  environment; sensitive values as secrets).

### Running a collection

**Run** in the toolbar runs every request in the workspace in tree order; **Run
folder…** in a folder's context menu runs just that folder. Choose the environment,
iterations, a delay between requests, whether to stop at the first failure, and whether
to send requests in parallel (up to 6 at once; values extracted by one request may not
reach the others in the same iteration, so use sequential runs for chains).

A request passes if all its assertions pass and its extractions find a value. Results
show live. **Details** shows a request's assertions; **Open in history** opens the full
request and response; **Copy results as Markdown** gives a table to paste into an issue
or chat. Values extracted during a run stay available afterwards.

## Settings

**Settings** (Ctrl+,): theme, font size and zoom; request defaults (timeout, redirects,
HTTP version, always using a fresh connection); extra sensitive headers; links to the
proxy and certificates, cookies, history, secret storage and global variables.

- **Proxy:** none, from the environment (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and
  `NO_PROXY`, upper or lower case; the default) or
  manual, with an optional username and password (kept in the OS keychain).
- **Certificates:** extra CA certificates for all workspaces, or for one workspace in
  its Network settings.

## Keyboard shortcuts

| Keys | Action |
|---|---|
| Ctrl+Enter | Send |
| Esc | Cancel the send; close History or a dialog |
| Ctrl+S | Save |
| Ctrl+N | New scratch request |
| Ctrl+W (or middle-click) | Close the tab |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+P | Go to a request by name, folder, method or URL |
| Ctrl+L | Focus the URL bar |
| Ctrl+H | History |
| Ctrl+, | Settings |
| Ctrl+= / Ctrl+- / Ctrl+0 | Zoom in / out / reset |
| Ctrl+Z / Ctrl+Y (or Ctrl+Shift+Z) | Undo / redo request edits |
| F2 | Rename the selected tree item |
| Del | Delete the selected tree item; close the focused tab |
| Arrow keys | Move through the tree; Left/Right collapse and expand |
| Enter, Home, End | Open the selected tree item; jump to the first or last |
| Shift+F10 or the Menu key | The selected tree item's context menu |

## Defaults

| Setting | Default | Where to change it |
|---|---|---|
| Timeout | 30 s (0 means none) | Settings › Requests, or a request's Settings tab |
| Redirects | followed, at most 10 | the same |
| HTTP version | auto (HTTP/2 when offered) | the same |
| TLS verification | on | only per request (the tab shows **TLS off**) |
| History | 10,000 entries, 90 days, 500 MB, bodies stored up to 1 MB each | **History settings…** |
| Theme, font size, zoom | system, 13 px, 100% | Settings › Appearance |

## Closing IronCall

Closing the window asks about unsaved workspace requests (**Save**, **Don't save** or
**Cancel**). Scratch requests are saved automatically, and open tabs come back next time.

## When something goes wrong

Settings › About & diagnostics has **Copy diagnostic info** (version, platform, a
settings summary and recent log lines, with secrets and your home folder path removed)
and **Open logs folder**. Please include the diagnostic info when you report a problem.

On Linux, IronCall turns off WebKitGTK's DMA-BUF renderer, which uses less memory and
scrolls more smoothly on most machines. If the window draws badly, try starting it with
`WEBKIT_DISABLE_DMABUF_RENDERER=0` to use WebKit's default renderer instead.
