# IronCall file format reference

A workspace is a folder of plain YAML files that you can commit to git. This page
describes every file IronCall reads and writes, and the rules it follows when writing
them. The schema versions described here are `@1` for every file type.

## Layout

```
my-api/
├── ironcall.yaml          # the collection: name, ID, variables, default headers and auth, order
├── environments/
│   ├── dev.yaml
│   └── prod.yaml
├── users/                 # a folder
│   ├── _folder.yaml       # folder variables, default headers and auth, order (optional)
│   ├── list-users.yaml    # a request
│   └── create-user.yaml
├── .gitattributes         # written when the workspace is created, if missing: `*.yaml text eol=lf`
└── .ironcall/             # local to this machine; contains a `.gitignore` with `*`
    ├── local.yaml         # active environment, workspace CAs
    └── secrets.json | secrets.enc   # only with the plaintext or encrypted-file backend
```

Commit everything except `.ironcall/`, which ignores itself.

Data that isn't part of the workspace lives in the app data folder,
`~/.local/share/io.ironcall.desktop/` (`$XDG_DATA_HOME/io.ironcall.desktop/` if set):

| File or folder | Contents |
|---|---|
| `settings.json` | App settings (theme, request defaults, proxy, history limits, …) |
| `globals.yaml` | Global variables (`ironcall/globals@1`; `vars` as in an [environment](#environment-environmentsyaml)) |
| `history.sqlite` | History of sends |
| `cookies/` | Cookie jars, one per workspace |
| `scratch/` | Scratch requests, one `<id>.yaml` per request (`ironcall/request@1`), and `folders.json`, the scratch folder layout (a missing or corrupt file shows a flat list) |
| `session.json`, `recent-workspaces.json` | Open tabs and the recent workspaces list |
| `secret-backends.json` | Where each workspace's secrets are stored on this machine |
| `spool/` | Response bodies while the app runs (cleared on exit) |
| `logs/` | Daily logs, `ironcall.YYYY-MM-DD.log`, kept 14 days |
| `webview/` | The UI's own storage (only with `IRONCALL_DATA_DIR`) |

The folder and the logs are readable only by you. `IRONCALL_DATA_DIR` moves all of this
to another folder (the end-to-end tests use it).

## Common rules

- **`schema`** names the file type and version, e.g. `ironcall/request@1`. When a
  version changes, older files will be migrated in memory and rewritten only when you
  save (every schema is still at version 1, so no migration exists yet). A file with a
  newer version than this IronCall understands opens read-only.
- **`id`** is a [ULID](https://github.com/ulid/spec). History, open tabs and run results
  refer to IDs, not paths, so moving and renaming are safe. If two files share an ID
  (say, after copying a file in a file manager), the copy gets a new ID and IronCall
  tells you. The copy is the one created or modified most recently.
- **`name`** is what IronCall shows. File names are chosen once, from the name, when the
  item is created; renaming only changes `name`, which keeps `git blame` and merges
  simple. Use *Rename file to match name* to rename the file too.
- **Paths inside files** use `/` and are relative to the workspace root (absolute paths
  are allowed for files outside it).
- **Unknown keys are kept.** A key IronCall doesn't know, at any level, survives a save.
- **Which files count:** every `*.yaml` file in the workspace is a request, except
  `ironcall.yaml`, `_folder.yaml` and the files in the root `environments/` folder.
  Files whose names start with `.`, and `.yml` files, are ignored.
- **Order:** `order` in `ironcall.yaml` and `_folder.yaml` lists child IDs. Items not in
  the list (for example, added by a merge) come last: folders first, then by name. IDs
  with no file are skipped and dropped the next time that folder's `order` is written.
- **Symbolic links are ignored** in a workspace: a linked folder could loop, show a file
  twice or let edits land outside the workspace. Use real folders.
- **Variables:** any string may contain `{{name}}` references (see
  [Variables](#variables)).

### How IronCall writes files

Saving produces one canonical form, so diffs stay small and saving an unchanged file
writes identical bytes:

- fixed key order, defaults left out (for example, `auth: {type: inherit}`)
- strings single-quoted (double-quoted when they need escaping: a backslash, a quote
  inside, a tab, or a newline in a one-line row); rows of tables (headers, params,
  assertions, extractions) as one-line flow maps; everything else, such as `auth` and
  `body`, as normal block maps
- multiline text as `|` blocks
- LF line endings and a trailing newline (CRLF is accepted on read)
- written atomically: a temporary file, then a rename

### Known limitations

- **Comments are lost** when IronCall saves a file, and hand formatting is normalised on
  the first save. Files you never save in IronCall are never rewritten.
- Unknown keys are written after the known ones.
- Unquoted values *inside unknown keys* keep YAML 1.1 typing: `no` becomes `false`.
  Known string fields keep their text.
- Strings that need escaping (regexes, Windows paths, text with quotes) are written
  double-quoted with escapes.
- Unquoted numbers in variables are normalised (`1.50` becomes `'1.5'`). Quote values
  whose exact text matters.

### Files IronCall can't read

A file with invalid YAML, leftover merge-conflict markers or schema errors shows as an
error node in the tree, with the line number. IronCall never overwrites or deletes it;
fix it in an editor (*Open in external editor*) and the tree updates.

## Request (`*.yaml`)

```yaml
schema: 'ironcall/request@1'
id: '01J9Z3K7T2V8Q4M6N0P5R1S3W7'
name: 'Create user'
description: |
  Creates a user. Markdown supported.
method: 'POST'
url: '{{baseUrl}}/users/:orgId/members'
pathParams:
- {name: 'orgId', value: '{{orgId}}'}
params:
- {name: 'notify', value: 'true', enabled: true}
headers:
- {name: 'Content-Type', value: 'application/json', enabled: true}
body:
  type: 'json'
  content: |
    {"name": "{{userName}}", "email": "{{$randomEmail}}"}
vars:
  userName: 'Ada'
extract:
- {var: 'userId', from: 'body', path: '$.id', sensitive: false}
assert:
- {target: 'status', op: 'eq', value: 201}
- {target: 'body', path: '$.email', op: 'exists'}
settings:
  timeoutMs: 30000
```

| Key | Meaning |
|---|---|
| `method` | Any HTTP method token, including custom ones (`PROPFIND`). |
| `url` | `:name` (or OpenAPI-style `{name}`) segments are path parameters, filled from `pathParams`. Query parameters can be in the URL or in `params`; IronCall keeps them in sync. |
| `pathParams` | `{name, value}` rows. |
| `params`, `headers` | `{name, value, enabled}` rows. Disabled rows aren't sent. In every kind of row, `value` defaults to empty and `enabled` to `true`. |
| `auth` | See [Auth](#auth). Absent means *inherit* from the folder or collection. |
| `body` | See [Bodies](#bodies). Absent means no body. |
| `vars` | Request-scope variables (never secret). |
| `extract` | See [Extraction](#extraction). |
| `assert` | See [Assertions](#assertions). |
| `settings` | Per-request overrides; absent keys use the app's defaults (Settings › Requests). |

`settings` keys: `timeoutMs` (0 for none), `followRedirects`, `maxRedirects`,
`verifyTls`, `httpVersion` (`auto`, `http1`, `http2`), `freshConnection` (don't reuse a
connection, so timings are measured cold), `encodeUrl` (`false` sends the URL exactly
as typed), `bypassProxy`.

### Bodies

`type` selects the kind:

| `type` | Keys |
|---|---|
| `json`, `text`, `xml` | `content` (text; may use variables) |
| `formUrlencoded` | `fields`: `{name, value, enabled}` rows |
| `multipart` | `parts`: each has `name`, `type` (`text` or `file`), `value` (text) or `path` (file), optional `contentType`, `enabled` |
| `binaryFile` | `path`, optional `contentType` |

Files are read when the request is sent, never stored in the workspace. A file outside
the workspace has to be picked in the body editor once per session before it can be
uploaded.

### Auth

Requests, folders and the collection can set `auth`. A request with no `auth` inherits
from the nearest folder that sets one, then the collection.

```yaml
auth:
  type: 'bearer'
  token: '{{token}}'
```

| `type` | Keys |
|---|---|
| `none` | Sends no auth, even if a folder sets some |
| `basic` | `username`, `password` |
| `bearer` | `token` |
| `apiKey` | `key` (header or parameter name), `value`, `in` (`header` or `query`) |

Keep credentials in secret variables, not literally in files. Either way, auth values are
masked in history, logs and masked cURL exports, and a redirect to another site never
carries them.

### Assertions

Each item checks the response after every send and in collection runs:

| Key | Meaning |
|---|---|
| `target` | `status`, `header`, `body` (JSONPath), `bodyText`, `duration` (ms), `size` (bytes) |
| `path` | JSONPath ([RFC 9535](https://www.rfc-editor.org/rfc/rfc9535)) for `body`, e.g. `$.items[0].id`, `$.books[?@.price > 50].title`. Several matches form a list. |
| `name` | Header name for `header` |
| `op` | `eq`, `neq`, `lt`, `lte`, `gt`, `gte`, `contains`, `matches` (regex), `exists`, `notExists`, `type`, `in` |
| `value` | The expected value; absent for `exists` / `notExists`. A list for `in`. For `type`: `string`, `number`, `integer`, `boolean`, `array`, `object` or `null`. |
| `enabled` | `false` skips it. |

Body values are typed: `201` and `'201'` differ. Headers, body text, status, duration
and size compare loosely (a header `Content-Length: 42` equals `42`), as does any
expected value written with `{{variables}}`, because variables are text.

### Extraction

Each item saves part of the response into a runtime variable for later requests:

| Key | Meaning |
|---|---|
| `var` | Variable name |
| `from` | `body` (JSONPath in `path`), `header` (`name`), `cookie` (`name`, from `Set-Cookie`), `regex` (`pattern` over the body text; the first group, or the whole match) |
| `sensitive` | Mask the value in the UI, history and logs. Always on for cookies and sensitive headers, and when the variable name or the body path contains a word like `token`, `secret`, `password`, `cookie`, `authorization` or `api_key`. |

## Folder (`_folder.yaml`)

Folders created in IronCall always get one. A folder made outside IronCall works without
it (its ID then lasts only for the session) until you set variables, headers, auth, a
description or an order, or rename or move it.

```yaml
schema: 'ironcall/folder@1'
id: '01J9Z3Q0A1B2C3D4E5F6G7H8J9'
name: 'Users'
description: 'Markdown'
vars:
  orgId: '42'
headers:
- {name: 'X-Team', value: 'core', enabled: true}
auth:
  type: 'bearer'
  token: '{{token}}'
order:
- '01J9Z3K7T2V8Q4M6N0P5R1S3W7'
```

`headers` are default headers for every request inside; a request's own header with
the same name wins.

## Collection (`ironcall.yaml`)

The same keys as a folder, with `schema: 'ironcall/collection@1'`. Its `id` identifies
the workspace (secrets are stored per workspace ID on each machine).

## Environment (`environments/*.yaml`)

```yaml
schema: 'ironcall/environment@1'
id: '01J9Z3M0A1B2C3D4E5F6G7H8J9'
name: 'Dev'
vars:
  baseUrl: 'https://dev.api.example.com'
  apiKey:
    secret: true          # the value is stored on this machine only
  region:
    value: 'ap-southeast-2'
    description: 'AWS region for the dev stack'
```

A variable is a plain string, or a map with `value`, `description` and `secret`. A
secret's value never appears in the file: it lives in the OS keychain (default), an
encrypted file or a plaintext file in `.ironcall/`, chosen per workspace. When someone
clones the workspace, IronCall lists the secrets they still need to enter.

## Local settings (`.ironcall/local.yaml`)

Never committed. Keys: `activeEnvironment` (an environment ID), `caCerts` (CA files
trusted for this workspace), `historyBodies` (`false` stops storing response bodies in
history).

`secretBackend` was written here before 1.0. It is no longer used, because a repo
could commit it to choose the plaintext file for everyone who clones it. The backend is
chosen per machine instead (`secret-backends.json` in the app data folder); if an old
`secretBackend` names another backend, IronCall says so and lets you switch.

## Variables

`{{name}}` is replaced when a request is sent. When a name is defined in several
places, the most specific wins:

1. Global (workspace menu or Settings › Network & storage › *Global variables…*)
2. Collection (`ironcall.yaml`)
3. Active environment
4. Folders, outermost first
5. Request (`vars`)
6. Runtime: values from `extract`, until the app closes

Values can refer to other variables (up to 10 levels). Built-in helpers produce a fresh
value per use:

| Helper | Value |
|---|---|
| `{{$uuid}}` | A random UUID v4 |
| `{{$ulid}}` | A random ULID |
| `{{$timestamp}}` | Unix time in seconds |
| `{{$isoTimestamp}}` | The current time, RFC 3339 in UTC |
| `{{$randomInt}}` | An integer from 0 to 1000 |
| `{{$randomEmail}}` | A random address at example.com |
| `{{$localEcho}}` | The URL of IronCall's built-in echo server |

Global variables can be secret, but their values can only be stored in the OS keychain
(they belong to no workspace, so there's no `.ironcall/` folder for a file).

## Versions

| File | Current schema |
|---|---|
| Request | `ironcall/request@1` |
| Folder | `ironcall/folder@1` |
| Collection | `ironcall/collection@1` |
| Environment | `ironcall/environment@1` |
| Global variables (`globals.yaml`, app data) | `ironcall/globals@1` |

Schema versions change independently of the app version; changes are listed in the
[CHANGELOG](../CHANGELOG.md).
